# ServeFS
ServeFS is a sqlite filesystem that allows you to serve local files and data (text and piped output from commands) to the web.

It uses a sqlite database to store file metadata and Tera templates to serve web pages that allow you to navigate through directories and view files.

## Overview
- servefs-server
  - a web server that serves the filesystem interface
  - allows you to graphically navigate directories and view files
- servefs-cli
  - command line interface to create/modify/view files and directories
- servefs-fuse
  - a fuse3 mountable filesystem using the sqlite backend
- servefs-lib
  - a library for interacting with the sqlite backend

## How to use
### ServeFS Server
```
Serves a sqlite based filesystem to the web

Usage: servefs-server [OPTIONS]

Options:
  -d, --db <DB>                      Location of database
      --db-prefix <DB_PREFIX>        Specify database table prefix
  -t, --templates <TEMPLATES>        Location of templates directory
      --dir-template <DIR_TEMPLATE>  Location of directory template inside templates directory
  -p, --port <PORT>                  
  -i, --ip <IP>                      
  -h, --help                         Print help information
  -V, --version                      Print version information
```

### ServeFS CLI
```
A cli interface for a sqlite based filesystem

Usage: servefs [OPTIONS] <COMMAND>

Commands:
  file  Operate on a file
  dir   Operate on a directory
  help  Print this message or the help of the given subcommand(s)

Options:
  -d, --db <DB>          Specify database location
  -p, --prefix <PREFIX>  Specify database table prefix
  -h, --help             Print help information
  -V, --version          Print version information
```

### ServeFS Fuse
```
A fuse3 interface for a sqlite based filesystem

Usage: servefs-fuse [OPTIONS] <MNT_PATH>

Arguments:
  <MNT_PATH>  Mount path

Options:
  -d, --db <DB>  Location of database
  -h, --help     Print help information
  -V, --version  Print version information
```

## Install
```bash
git clone https://github.com/ellabellla/servefs.git
cd servefs
cargo install --path ./servefs-server
cargo install --path ./servefs-cli
cargo install --path ./servefs-fuse
```

### Installing ServeFS Fuse
To install ServeFS Fuse you must first install fuse 3 for your distribution of linux.
e.g.
```bash
sudo pacman -S fuse3
```

## Uninstall
```bash
cargo uninstall servefs-fuse
cargo uninstall servefs-server
cargo uninstall servefs
```

## License
This software is provided under the MIT license. Click [here](LICENSE) to view.