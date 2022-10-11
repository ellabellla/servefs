use std::{time::{Duration, UNIX_EPOCH, Instant}, path::PathBuf, str, str::FromStr, fs, collections::HashMap, process::{Stdio}, os::{unix::prelude::{PermissionsExt}, linux::fs::MetadataExt}};
use fuser::{Filesystem, FileAttr, FileType, MountOption, consts::FOPEN_DIRECT_IO};
use libc::{ENOENT};
use rand::{rngs::ThreadRng, Rng};
use servefs_lib::{FSConnection, Directory, File};
use sqlx::Row;
use tokio::{runtime::Runtime, io::{BufReader, AsyncBufReadExt}};

const TTL: Duration = Duration::from_secs(1);
const INODE_SPLIT:u64 = std::u64::MAX / 2;

fn calc_size(size: usize, offset:usize, data: &Vec<u8>) -> usize {
    let size = offset as usize + size as usize;
    if size > data.len() {
        data.len()
    } else {
        size
    }
}
fn file_id_to_ino(id:i64) -> u64 {
    (id as u64) + INODE_SPLIT
}

async fn exec(command: &str, rt: &Runtime) -> Vec<u8>{
    if let Ok(mut child) = tokio::process::Command::new("bash")
        .arg("-c")
        .arg(&command)
        .stdout(Stdio::piped())
        .spawn() {
            let stdout = match child.stdout.take() {
                Some(stdout) => stdout,
                None => return vec![],
            };
            let mut reader = BufReader::new(stdout).lines();
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
    println!("get data {}", data);
    match ftype {
        servefs_lib::FileType::File => match fs::read(&data) {
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
    store: HashMap<u64, Vec<u8>>,
    rng: ThreadRng,
}

impl Store {
    pub fn insert(&mut self, file: &File, rt: &Runtime, fs_conn: &FSConnection) -> u64 {
        let data = rt.block_on(file.read(fs_conn))
            .map(|(data, ftype)| {
                servefs_lib::FileType::from_str(&ftype)
                    .map(|ftype| get_data(&data, &ftype, rt))
                    .unwrap_or(vec![0x0])
            }).unwrap_or(vec![0x0]);
        let mut fh = self.rng.gen::<u64>();
        while self.store.contains_key(&fh) {
            fh = self.rng.gen();
        }
        self.store.insert(fh, data);
        println!("insert {} into {}", rt.block_on(file.get_id(&fs_conn)).unwrap_or(-1), fh);
        fh
    }

    pub fn get(&self, fh: &u64) -> Option<&Vec<u8>> {
        if let Some(data) = self.store.get(&fh){
            return Some(data);
        }

        return None
    }

    pub fn contains(&self, fh: &u64) -> bool  {
        self.store.contains_key(fh)
    } 

    pub fn remove(&mut self, fh: &u64) {
        self.store.remove(fh);
    }
}

struct ServeFS {
    fs_conn: FSConnection,
    rt: Runtime,
    store: Store,
}

impl ServeFS {
    fn create_file_attr(&self, ino: u64, size: u64, file: &File) -> FileAttr {
        let (data, ftype) = self.rt.block_on(file.read(&self.fs_conn))
            .map(|(data, ftype)| {
                servefs_lib::FileType::from_str(&ftype)
                    .map(|ftype| (data, ftype))
                    .unwrap_or(("".to_string(), servefs_lib::FileType::Exec))
            }).unwrap_or(("".to_string(), servefs_lib::FileType::Exec));
        
        match ftype {
            servefs_lib::FileType::File =>if let Ok(meta) = fs::File::open(&data).and_then(|file| file.metadata()) {
                return FileAttr{
                    ino: ino,
                    size: meta.st_size(),
                    blocks: 1,
                    atime: meta.accessed().unwrap_or(UNIX_EPOCH),
                    mtime: meta.modified().unwrap_or(UNIX_EPOCH),
                    ctime: UNIX_EPOCH,
                    crtime: meta.created().unwrap_or(UNIX_EPOCH),
                    kind: FileType::RegularFile,
                    perm: meta.permissions().mode() as u16,
                    nlink: 1,
                    uid: meta.st_uid(),
                    gid: meta.st_gid(),
                    rdev: meta.st_rdev() as u32,
                    flags:0,
                    blksize: 512,
                    padding: 0,
                };
            },
            servefs_lib::FileType::Text => return FileAttr{
                ino: ino,
                size: data.len() as u64,
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
            },
            servefs_lib::FileType::Exec => (),
        }
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
        println!("lookup {} {}", parent, name.to_string_lossy());
        if parent >= INODE_SPLIT {
            reply.error(ENOENT)
        } else {
            match self.rt.block_on(Directory::from_id(parent as i64, &self.fs_conn)) {
                Ok(parent) => {
                    let name = match name.to_str() {
                        Some(name) => name,
                        None => {
                            reply.error(ENOENT);
                            return;
                        },
                    };
                    let file = parent.file(name);
                    
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
                            &self.create_file_attr(file_id_to_ino(id), 1, &file), 
                            0);
                    } else {
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
                        &self.create_file_attr(ino, 1, &file));
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
                Ok(_) => {
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

    fn open(&mut self, _req: &fuser::Request<'_>, ino: u64, _flags: i32, reply: fuser::ReplyOpen) {
        if ino >=  INODE_SPLIT {
            let ino = ino - INODE_SPLIT;
            match self.rt.block_on(File::from_id(ino as i64, &self.fs_conn)) {
                Ok(file) => {
                    let fh = self.store.insert( &file, &self.rt, &self.fs_conn);
                    println!("created fh {}", fh);
                    reply.opened(fh, FOPEN_DIRECT_IO);
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
                    let empty = vec![0x0];
                    let mut tmp = vec![];
                    let data = self.store.get(&fh).or_else(|| {
                        self.rt.block_on(file.read(&self.fs_conn)).map(|(data_str, ftype)| {
                            servefs_lib::FileType::from_str(&ftype).map(|ftype| {
                                tmp = get_data(&data_str, &ftype, &self.rt);
                                &tmp
                            }).unwrap_or(&empty)
                        }).ok()
                    }).unwrap_or(&empty);
                    let size = calc_size(size as usize, offset as usize, data);
                    if offset as usize > data.len() {
                        reply.data(&vec![]);
                        return;
                    }
                    println!("read {} {} {} {}", fh, ino, offset, size);
                    reply.data(&data[offset as usize..size]);
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
            fh: u64,
            _flags: i32,
            _lock_owner: Option<u64>,
            _flush: bool,
            reply: fuser::ReplyEmpty,
        ) {
        if ino >=  INODE_SPLIT {
            println!("released fh {}", fh);
            self.store.remove(&fh);
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
    let servefs = ServeFS{ fs_conn, rt, store: Store { store: HashMap::new(), rng: rand::thread_rng() } };
    fuser::mount2(servefs, "./mnt", &options).unwrap();
}
