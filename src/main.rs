use sigmaos_fuse2::{FsError, Vfs};
use std::path::PathBuf;
use std::process::exit;

fn usage() -> ! {
    eprintln!("usage: sigmaos_fuse2 [--root DIR] <cmd> [args]");
    eprintln!("commands:");
    eprintln!("  mount SOURCE TARGET | umount TARGET | mounts");
    eprintln!("  mkdir PATH | write PATH [TEXT] | read PATH | ls PATH | stat PATH");
    eprintln!("  rm FILE | rmdir DIR");
    exit(2);
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut root = std::env::var("SIGMA_FUSE_ROOT").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        PathBuf::from(home).join(".local/share/sigmaos/fuse2").to_string_lossy().to_string()
    });
    if args.next().as_deref() == Some("--root") {
        root = args.next().unwrap_or_else(|| usage());
    }
    let Some(cmd) = args.next() else { usage() };
    let rest: Vec<String> = args.collect();
    let vfs = match Vfs::open(&root) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error: {e}");
            exit(1);
        }
    };

    let result = (|| -> Result<(), FsError> {
        match cmd.as_str() {
            "mount" => {
                if rest.len() < 2 {
                    usage();
                }
                let mut v = vfs;
                v.mount(&rest[0], &rest[1])?;
                println!("mounted {} at {}", rest[0], rest[1]);
            }
            "umount" => {
                let target = rest.first().map(String::as_str).unwrap_or("");
                let mut v = vfs;
                println!("unmounted {}", v.umount(target)?);
            }
            "mounts" => {
                for (t, s) in vfs.mounts() {
                    println!("{t}\t{s}");
                }
            }
            "mkdir" => vfs.mkdir(rest.first().map(String::as_str).unwrap_or("/"))?,
            "write" => {
                let path = rest.first().map(String::as_str).unwrap_or("");
                let text = rest.get(1).cloned().unwrap_or_default();
                vfs.write_file(path, text.as_bytes())?;
                println!("wrote {} bytes", text.len());
            }
            "read" => {
                let data = vfs.read_file(rest.first().map(String::as_str).unwrap_or(""))?;
                print!("{}", String::from_utf8_lossy(&data));
            }
            "ls" => {
                for name in vfs.list_dir(rest.first().map(String::as_str).unwrap_or("/"))? {
                    println!("{name}");
                }
            }
            "stat" => {
                let st = vfs.stat(rest.first().map(String::as_str).unwrap_or("/"))?;
                println!("{} dir={} size={} mtime={}", st.path, st.is_dir, st.size, st.modified_secs);
            }
            "rm" => vfs.remove_file(rest.first().map(String::as_str).unwrap_or(""))?,
            "rmdir" => vfs.remove_dir(rest.first().map(String::as_str).unwrap_or(""))?,
            _ => usage(),
        }
        Ok(())
    })();

    if let Err(e) = result {
        eprintln!("error: {e}");
        exit(1);
    }
}
