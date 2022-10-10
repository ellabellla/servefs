use std::{time::{Duration, UNIX_EPOCH, SystemTime, Instant}, path::PathBuf, str, str::FromStr, fs, collections::HashMap, ops::Sub, process::{Stdio}};
use fuser::{Filesystem, FileAttr, FileType, MountOption, consts::FOPEN_DIRECT_IO};
use libc::{ENOENT};
use servefs_lib::{FSConnection, Directory, File};
use sqlx::Row;
use tokio::{runtime::Runtime, io::{BufReader, AsyncBufReadExt, AsyncReadExt}};

const TTL: Duration = Duration::from_secs(1);
const INODE_SPLIT:u64 = std::u64::MAX / 2;

fn file_id_to_ino(id:i64) -> u64 {
    (id as u64) + INODE_SPLIT
}

async fn exec(command: &str, rt: &Runtime) -> Vec<u8>{
    if let Ok(mut child) = tokio::process::Command::new("bash")
        .arg("-c")
        .arg(&command)
        .stdout(Stdio::piped())
        .spawn() {
            let mut reader = BufReader::new(child.stdout.take().unwrap()).lines();
            let mut buffer = String::new();
            rt.spawn( async move {
                tokio::time::timeout(Duration::from_secs(1), child.wait()).await
            });
            let start = Instant::now();

            while start.elapsed() < Duration::from_secs(1) {
                if let Ok(Ok(line)) = async {tokio::time::timeout(Duration::from_millis(100), reader.next_line()).await}.await {
                    if let None = line {
                        break;
                    }
                    buffer.extend(line);
                    buffer.push('\n');
                }

            }
            
            buffer.as_bytes().to_vec()
        } else {
            vec![0x0]
        }
}

fn get_data(data: &str, ftype: &servefs_lib::FileType, rt: &Runtime) -> Vec<u8> {
    println!("data {}", data);
    match ftype {
        servefs_lib::FileType::File => match fs::read(data) {
            Ok(file) => file,
            Err(e) => {
                println!("{:?}", e);
                vec![0x0]
            },
        },
        servefs_lib::FileType::Text => data.as_bytes().to_vec(),
        servefs_lib::FileType::Exec => rt.block_on(exec(&data, rt)),
    }
}

struct Store {
    store: HashMap<u64, (Instant, Vec<u8>)>,
    counts: HashMap<u64, u64>,
    timeout: Duration,
}

impl Store {
    pub fn update(&mut self, id: &u64, file: &File, rt: &Runtime, fs_conn: &FSConnection) {
        println!("update id{}", id);
        let time = self.store.get(id).map(|(t,_)| t.clone()).unwrap_or_else(|| {
            if let Some(count) = self.counts.get_mut(&id) {
                *count = *count + 1;
            } else {
                self.counts.insert(*id, 1);
            }
            Instant::now().sub(self.timeout)
        });
        if time.elapsed() >= self.timeout {
            println!("updated {:?}", time.elapsed());
            let data = rt.block_on(file.read(fs_conn))
                .map(|(data, ftype)| {
                    servefs_lib::FileType::from_str(&ftype)
                        .map(|ftype| get_data(&data, &ftype, rt))
                        .unwrap_or(vec![0x0])
                }).unwrap_or(vec![0x0]);
            self.store.insert(id.clone(), (Instant::now(), data));

        }
    }

    pub fn get(&self, id: &u64) -> Option<&Vec<u8>> {
        if let Some((_, data)) = self.store.get(&id){
            return Some(data);
        }

        return None
    }

    pub fn contains(&self, id: &u64) -> bool  {
        self.store.contains_key(id)
    } 

    pub fn remove(&mut self, id: &u64) {
        if let Some(count) = self.counts.get_mut(&id) {
            *count = *count - 1;
            if *count == 0 {
                self.store.remove(id);
            }
        } else {
            self.store.remove(id);
        }
    }
}

struct ServeFS {
    fs_conn: FSConnection,
    rt: Runtime,
    store: Store,
}

impl ServeFS {
    fn create_file_attr(&self, ino: u64, size: u64) -> FileAttr {
        /*let size = self.rt.block_on(file.read(&self.fs_conn))
            .map(|(data, ftype)| {
                servefs_lib::FileType::from_str(&ftype)
                    .map(|ftype| get_data(&data, &ftype).len() as u64)
                    .unwrap_or(0)
            }).unwrap_or(0);*/
        
        //println!("{} size", size);
        FileAttr{
            ino: ino,
            size: size,
            blocks: 1,
            atime: UNIX_EPOCH,
            mtime: UNIX_EPOCH,
            ctime: UNIX_EPOCH,
            crtime: UNIX_EPOCH,
            kind: FileType::RegularFile,
            perm: 0o644,
            nlink: 1,
            uid: unsafe {libc::geteuid() as u32},
            gid: unsafe {libc::getegid() as u32},
            rdev: 0,
            flags: 0,
            blksize: 512,
            padding: 0,
        }
    }
    
    fn create_dir_attr(&self, ino: u64) -> FileAttr {
        FileAttr{
            ino: ino,
            size: 0,
            blocks: 0,
            atime: UNIX_EPOCH,
            mtime: UNIX_EPOCH,
            ctime: UNIX_EPOCH,
            crtime: UNIX_EPOCH,
            kind: FileType::Directory,
            perm: 0o755,
            nlink: 2,
            uid: unsafe {libc::geteuid() as u32},
            gid: unsafe {libc::getegid() as u32},
            rdev: 0,
            flags: 0,
            blksize: 512,
            padding: 0,
        }
    }
    
}

impl Filesystem for ServeFS {
    fn lookup(&mut self, _req: &fuser::Request<'_>, parent: u64, name: &std::ffi::OsStr, reply: fuser::ReplyEntry) {
        println!("{} {}", parent, name.to_string_lossy());
        if parent >= INODE_SPLIT {
            reply.error(ENOENT)
        } else {
            match self.rt.block_on(Directory::from_id(parent as i64, &self.fs_conn)) {
                Ok(parent) => {
                    let file = parent.file(name.to_str().unwrap());
                    
                    if self.rt.block_on(file.exists(&self.fs_conn)).unwrap_or(false) {
                        let id = match self.rt.block_on(file.get_id(&self.fs_conn)) {
                            Ok(id) => id,
                            Err(e) => {
                                println!("{:?}", e);
                                reply.error(ENOENT);
                                return;
                            }
                        };
                        reply.entry(
                            &TTL, 
                            &self.create_file_attr(file_id_to_ino(id), 1), 
                            0);
                    } else {
                        if let Some(name) = name.to_str() {
                            if let Ok(dir) = parent.dir(&name) {
                                if self.rt.block_on(dir.exists(&self.fs_conn)).unwrap_or(false) {
                                    let id = match self.rt.block_on(dir.get_id(&self.fs_conn)) {
                                        Ok(id) => id,
                                        Err(e) => {
                                            println!("{:?}", e);
                                            reply.error(ENOENT);
                                            return;
                                        }
                                    };
            
                                    reply.entry(
                                        &TTL, 
                                        &self.create_dir_attr(id as u64), 
                                        0);
                                }
                            }
                        }
                    }
                },
                Err(e) => {
                    println!("{:?}", e);
                    reply.error(ENOENT)
                },
            }
        }
    }

    fn getattr(&mut self, _req: &fuser::Request<'_>, ino: u64, reply: fuser::ReplyAttr) {
        if ino >=  INODE_SPLIT {
            let ino = ino - INODE_SPLIT;
            match self.rt.block_on(File::from_id(ino as i64, &self.fs_conn)) {
                Ok(file) => {
                    reply.attr(
                        &TTL, 
                        &self.create_file_attr(ino, 1));
                }
                Err(e) => {println!("{:?}", e);
                reply.error(ENOENT)},
            }
        } else {
            match self.rt.block_on(Directory::from_id(ino as i64, &self.fs_conn)) {
                Ok(_) => {
                    reply.attr(
                        &TTL, 
                        &self.create_dir_attr(ino));
                }
                Err(e) => {println!("{:?}", e);
                reply.error(ENOENT)},
            }
        }
    }

    fn access(&mut self, _req: &fuser::Request<'_>, ino: u64, _mask: i32, reply: fuser::ReplyEmpty) {
        if ino >=  INODE_SPLIT {
            let ino = ino - INODE_SPLIT;
            match self.rt.block_on(File::from_id(ino as i64, &self.fs_conn)) {
                Ok(file) => {
                    reply.ok()
                }
                Err(e) => {println!("{:?}", e);
                    reply.error(ENOENT)},
            }
        } else {
            match self.rt.block_on(Directory::from_id(ino as i64, &self.fs_conn)) {
                Ok(_) => {
                    reply.ok()
                }
                Err(e) => {println!("{:?}", e);
                    reply.error(ENOENT)},
            }
        }
    }

    fn open(&mut self, req: &fuser::Request<'_>, ino: u64, _flags: i32, reply: fuser::ReplyOpen) {
        if ino >=  INODE_SPLIT {
            let ino = ino - INODE_SPLIT;
            match self.rt.block_on(File::from_id(ino as i64, &self.fs_conn)) {
                Ok(file) => {
                    self.store.update(&ino, &file, &self.rt, &self.fs_conn);
                    reply.opened(ino, FOPEN_DIRECT_IO);
                },
                Err(e) => {
                    println!("{:?}", e);
                    reply.error(ENOENT)
                },
            }
        } else {
            reply.error(ENOENT)
        }
    }

    fn read(
        &mut self,
        _req: &fuser::Request<'_>,
        ino: u64,
        fh: u64,
        offset: i64,
        size: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: fuser::ReplyData,
    ) {
        if ino >=  INODE_SPLIT {
            let ino = ino - INODE_SPLIT;
            match self.rt.block_on(File::from_id(ino as i64, &self.fs_conn)) {
                Ok(file) => {
                    if self.store.contains(&fh) {
                        println!("fh used {}", ino);
                        self.store.update(&ino, &file, &self.rt, &self.fs_conn);
                        let empty = vec![0x0];
                        let data = self.store.get(&fh).unwrap_or(&empty);
                        let size = if size > data.len() as u32 {
                            data.len()
                        } else {
                            size as usize
                        };
                        reply.data(&data[offset as usize..size])
                    } else if let Ok((data, ftype)) = self.rt.block_on(file.read(&self.fs_conn)) {
                        if let Ok(ftype) = servefs_lib::FileType::from_str(&ftype) {
                            let data = get_data(&data, &ftype, &self.rt);

                            if data.len() == 0{
                                println!("not found {}", ino);
                            }

                            let size = if size > data.len() as u32 {
                                data.len()
                            } else {
                                size as usize
                            };

                            reply.data(&data[offset as usize..size])
                        }
                    }
                },
                Err(e) => {
                    println!("{:?}", e);
                    reply.error(ENOENT)
                },
            }
        } else {
            reply.error(ENOENT)
        }
    }

    fn release(
            &mut self,
            _req: &fuser::Request<'_>,
            ino: u64,
            _fh: u64,
            _flags: i32,
            _lock_owner: Option<u64>,
            _flush: bool,
            reply: fuser::ReplyEmpty,
        ) {
        if ino >=  INODE_SPLIT {
            let ino = ino - INODE_SPLIT;
            self.store.remove(&ino);
            reply.ok()
        }
    }

    fn readdir(
        &mut self,
        _req: &fuser::Request<'_>,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: fuser::ReplyDirectory,
    ) {
        if ino >= INODE_SPLIT {
            reply.error(ENOENT);
            return;
        }

        let mut entries = vec![
            (1, FileType::Directory, ".".to_string()),
            (1, FileType::Directory, "..".to_string()),
        ];

        if let Ok(dir) = self.rt.block_on(Directory::from_id(ino as i64, &self.fs_conn)) { 
            if let Ok((file, dirs)) = self.rt.block_on(dir.contents(&self.fs_conn)) {
                entries.extend(file.iter()
                    .map(|row| 
                        (
                            file_id_to_ino(row.get::<i64, &str>("id")), 
                            FileType::RegularFile, row.get::<String, &str>("name")
                        )
                    ).collect::<Vec<(u64, FileType, String)>>());
                entries.extend(dirs.iter()
                    .filter_map(|row| {
                        let path = PathBuf::from_str(&row.get::<String, &str>("directory")).ok()?;
                        let name = path.components().last()?.as_os_str().to_string_lossy().to_string();
                        Some((row.get::<i64, &str>("id") as u64, FileType::Directory, name))
                    }).collect::<Vec<(u64, FileType, String)>>());
            }
        }

        //println!("{:?}", entries);

        for (i, entry) in entries.into_iter().enumerate().skip(offset as usize) {
            // i + 1 means the index of the next entry
            if reply.add(entry.0, (i + 1) as i64, entry.1, entry.2) {
                break;
            }
        }
        reply.ok();
    }
}

fn main() {
    let options = vec![MountOption::RO, MountOption::FSName("hello".to_string()), MountOption::AutoUnmount];
    let rt = Runtime::new().unwrap();
    let fs_conn =  rt.block_on(FSConnection::new("sqlite:///home/ella/.config/servefs/fs.db", "servefs_", true)).unwrap();
    let servefs = ServeFS{ fs_conn, rt, store: Store { store: HashMap::new(), counts: HashMap::new(), timeout: Duration::from_secs(1) } };
    fuser::mount2(servefs, "./mnt", &options).unwrap();
}
