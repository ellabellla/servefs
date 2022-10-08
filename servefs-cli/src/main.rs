use std::{path::PathBuf, fs};

use clap::{Parser, command, Subcommand, ValueEnum};
use servefs_lib::{FSConnection, File, FSError, Directory};
use sqlx::Row;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[clap(short,long)]
    /// Specify database location
    db: Option<String>,

    #[clap(short,long)]
    /// Specify database table prefix
    prefix: Option<String>,

    #[clap(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Operate on a file
    File { 
        #[clap(subcommand)]
        file_command: FileCommands,
        /// path to file
        path: PathBuf,
     },
    /// Operate on a directory
    Dir { 
        #[clap(subcommand)]
        directory_command: DirCommands,
        /// Path to directory
        path: PathBuf,
     },
}

#[derive(Subcommand, Debug)]
enum FileCommands {
    /// Check if file exists
    Exists,
    /// Make a file
    Mk {
        data: String, 
        #[arg(value_enum)]
        ftype: FileType 
    },
    /// Delete file
    Del,
    /// Rename file
    Rn {
        name: String
    },
    /// Move file
    Mv {
        directory: PathBuf
    },
    /// Read file
    Read,
    // Write to file
    Write {
        data: String, 
        #[arg(value_enum)]
        ftype: FileType 
    },
}

#[derive(Subcommand, Debug)]
enum DirCommands {
    /// Check if directory exists
    Exists,
    /// Make a directory
    Mk,
    /// Delete directory
    Del,
    /// Rename directory
    Rn {
        name: String
    },
    /// Move directory
    Mv {
        directory: PathBuf
    },
    /// Read contents of directory
    Contents {
        /// Show contents of directory recursively 
        #[arg(short, long)]
        recursive: bool
    }
}

#[derive(ValueEnum, Clone, Debug)]
enum FileType {
    Text,
    Exec,
    File,
}

impl From<servefs_lib::FileType> for FileType {
    fn from(ftype: servefs_lib::FileType) -> Self {
        match ftype {
            servefs_lib::FileType::File => FileType::File,
            servefs_lib::FileType::Text => FileType::Text,
            servefs_lib::FileType::Exec => FileType::Exec,
        }
    }
}

impl Into<servefs_lib::FileType> for FileType {
    fn into(self) -> servefs_lib::FileType {
        match self {
            FileType::Text => servefs_lib::FileType::Text,
            FileType::Exec => servefs_lib::FileType::Exec,
            FileType::File => servefs_lib::FileType::File,
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), FSError> {
    let default_config_dir = "servefs/";
    let default_db_path_prefix = "sqlite://";
    let default_db_name = "fs.db";
    let default_db_prefix = "servefs_";

    let mut config = dirs::config_dir().expect("Could not find config path.");
    config.push(default_config_dir);
    fs::create_dir_all(&config).expect("Couldn't find a default config location");

    let args = Args::parse();
    
    let db_loc = match args.db {
        Some(db_loc) => db_loc,
        None => {
            let mut db_loc = config.clone();
            db_loc.push(default_db_name);
            let db_loc = db_loc.to_str().expect("Couldn't find a default db location").to_string();
            format!("{}{}", default_db_path_prefix, db_loc)
        },
    };

    let db_prefix = match args.prefix {
        Some(prefix) => prefix,
        None => default_db_prefix.to_string(),
    };

    let fs_conn = FSConnection::new(&db_loc, &db_prefix, true).await.map_err(|e| FSError::SqlX(e))?;

    match args.command {
        Commands::File { file_command, path } => {
            let mut file = File::new(path)?;
            match file_command {
                FileCommands::Exists => {
                    println!("{}", file.exists(&fs_conn).await.map_err(|e| FSError::SqlX(e))?);
                }
                FileCommands::Mk { data, ftype } => {
                    file.mk(&data, &ftype.into(), &fs_conn).await.map_err(|e| FSError::SqlX(e))?;
                },
                FileCommands::Del => {
                    file.del(&fs_conn).await.map_err(|e| FSError::SqlX(e))?;
                },
                FileCommands::Rn { name } => {
                    file.rename(&name, &fs_conn).await.map_err(|e| FSError::SqlX(e))?;
                },
                FileCommands::Mv { directory } => {
                    let dir = Directory::new(directory)?;
                    file.mv(dir, &fs_conn).await.map_err(|e| FSError::SqlX(e))?;
                },
                FileCommands::Read => {
                    let (data, ftype) = file.read(&fs_conn).await.map_err(|e| FSError::SqlX(e))?;
                    println!("{}, {}", data, ftype);
                },
                FileCommands::Write { data, ftype } => {
                    file.write(&data, ftype.into(), &fs_conn).await.map_err(|e| FSError::SqlX(e))?;
                },
            };
        },
        Commands::Dir { directory_command, path } => {
            let mut dir = Directory::new(path)?;
            match directory_command {
                DirCommands::Exists => {
                    dir.exists(&fs_conn).await.map_err(|e| FSError::SqlX(e))?;
                },
                DirCommands::Mk => {
                    dir.mk(&fs_conn).await.map_err(|e| FSError::SqlX(e))?;
                },
                DirCommands::Del => {
                    dir.del(&fs_conn).await.map_err(|e| FSError::SqlX(e))?;
                },
                DirCommands::Rn { name } => {
                    let new_path = dir.rename(&name)?;
                    let new_dir = Directory::new(new_path)?;
                    dir.mv(&new_dir, &fs_conn).await.map_err(|e| FSError::SqlX(e))?;
                    
                },
                DirCommands::Mv { directory } => {
                    let new_dir = Directory::new(directory)?;
                    dir.mv(&new_dir, &fs_conn).await.map_err(|e| FSError::SqlX(e))?;
                }
                DirCommands::Contents { recursive } => {
                    let (files, dirs) = if recursive {
                        dir.recurse(&fs_conn).await.map_err(|e| FSError::SqlX(e))?
                    } else {
                        dir.contents(&fs_conn).await.map_err(|e| FSError::SqlX(e))?
                    };

                    let dirs = dirs
                        .iter()
                        .map(|row| row.get::<String, &str>("directory"))
                        .map(|name| name);
                    let files = files
                        .iter()
                        .map(|row| row.get::<String, &str>("name"))
                        .map(|name| name);

                    dirs.chain(files).for_each(|n| println!("{}", n));
                }
            };
        }
    };
    
    Ok(())
}
