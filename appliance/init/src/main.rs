use std::fs::{self, File};
use std::io::{self, Read, BufReader, BufRead};
use std::path::Path;
use std::process::Command;
use std::thread::sleep;
use std::time::Duration;
use std::path::PathBuf;

use anyhow::Context;
use tracing::{event, Level};
use tracing_subscriber::filter::LevelFilter as TracingLevelFilter;
use tracing_subscriber::FmtSubscriber;

const NONE_OF_SLICE: Option<&'static [u8]> = None;


#[cfg(not(target_os = "linux"))]
fn main() {
    panic!("linux only");
}

#[cfg(target_os = "linux")]
fn main() {
    use nix::sys::reboot::{reboot, RebootMode};

    if let Err(err) = main2() {
        eprintln!("{:?}", err);

        reboot(RebootMode::RB_POWER_OFF).expect("Restart failed");
    }
}

fn get_cmdline_presence<'a>(props: &[(&str, Option<&'a str>)], key: &str) -> bool {
    for prop in props {
        if prop.0 == key {
            return true;
        }
    }
    false
}

// these should really be &[u8] or something, but let's use &str until it causes
// us trouble?
fn get_cmdline_value<'a>(props: &[(&str, Option<&'a str>)], key: &str) -> Option<&'a str> {
    for prop in props {
        if prop.0 == key {
            return prop.1;
        }
    }
    None
}

fn partition_def_to_path(value: &str) -> PathBuf {
    const LABEL_PREFIX: &str = "LABEL=";
    const UUID_PREFIX: &str = "UUID=";

    if value.starts_with(LABEL_PREFIX) {
        let mut out: PathBuf = "/dev/disk/by-label".into();
        out.push(&value[LABEL_PREFIX.len()..]);
        return out;
    }

    if value.starts_with(UUID_PREFIX) {
        let mut out: PathBuf = "/dev/disk/by-uuid".into();
        out.push(&value[UUID_PREFIX.len()..]);
        return out;
    }

    return value.into();
}

#[test]
fn test_partition_def_to_path() {
    assert_eq!(
        partition_def_to_path("UUID=1234"),
        PathBuf::from("/dev/disk/by-uuid/1234"));

    assert_eq!(
        partition_def_to_path("LABEL=2345"),
        PathBuf::from("/dev/disk/by-label/2345"));
    
    assert_eq!(
        partition_def_to_path("/dev/sda1"),
        PathBuf::from("/dev/sda1"));
}

// enum Partition<'a> {
//     ByLabel(&'a Path),
//     ByUuid(&'a Path),
//     ByPath(&'a Path),
// }

// impl Partition {
//     fn 

//     fn parse_from(&'a str) -> Partition<'a> {
//         const LABEL_PREFIX: &str = "LABEL=";
//         const UUID_PREFIX: &str = "UUID=";

//         if value.starts_with(LABEL_PREFIX) {
//             return Partition::ByLabel(From::from(&value[LABEL_PREFIX.len()..]));
//         }

//         if value.starts_with(UUID_PREFIX) {
//             return Partition::ByUuid(From::from(&value[UUID_PREFIX.len()..]));
//         }

//         return Partition::ByPath(From::from(&value));
//     }

//     fn to_path(&self) -> PathBuf {
//         match *self {
//             Partition::ByLabel(p) => {
//                 "/dev/disk/by-label"
//             },
//             Partition::ByUuid(p) => (),
//             Partition::ByPath(p) => (),
//         }
//     }
// }

#[cfg(target_os = "linux")]
fn main2() -> anyhow::Result<()> {
    use nix::NixPath;
    use std::fmt::Debug;
    use nix::sys::reboot::{reboot, RebootMode};
    use nix::mount::{MsFlags, mount as nix_mount};
    use nix::mount::{umount2, MntFlags};

    fn mount<P1, P2, P3, P4>(
        source: Option<&P1>,
        target: &P2,
        fstype: Option<&P3>,
        flags: MsFlags,
        data: Option<&P4>)
    -> anyhow::Result<()>
        where P1: ?Sized + NixPath + Debug,
            P2: ?Sized + NixPath + Debug,
            P3: ?Sized + NixPath + Debug,
            P4: ?Sized + NixPath + Debug,
    {
        nix_mount(source, target, fstype, flags, data)
            .with_context(|| {
                let source = source
                    .map(|v| format!("source:{:?}", v))
                    .unwrap_or_else(|| "none".to_string());
                let fstype = fstype
                    .map(|v| format!("{:?}", v))
                    .unwrap_or_else(|| "auto".to_string());

                format!("failed to mount {} fstype:{} -> {:?}", source, fstype, target)
            })
    }

    std::env::set_var("RUST_BACKTRACE", "full");

    // PATH=/nix/store/93l9n0msl71fw2ba3wbj9y5nx5mbzd8p-system-path/bin
    let config = File::open("/init.config")?;
    let config = BufReader::new(config);
    for line in config.lines() {
        let line = line?;

        if line.starts_with("LINK ") {
            let mut chunks = line[5..].splitn(2, ' ');
            let target = chunks.next().expect("first element will exist, even if empty");
            let source = chunks.next().expect("LINK needs two args");
            std::os::unix::fs::symlink(source, target)?;
        }

        if line.starts_with("MKDIR ") {
            let dirname = &line[6..];
            if let Err(err) = fs::create_dir(dirname) {
                println!("creating {} failed: {}", dirname, err);
            }
        }
    }

    if let Err(err) = fs::create_dir("/proc") {
        println!("creating /proc failed: {}", err);
    }
    if let Err(err) = fs::create_dir("/sys") {
        println!("creating /sys failed: {}", err);
    }
    if let Err(err) = fs::create_dir("/run") {
        println!("creating /run failed: {}", err);
    }

    mount(
        NONE_OF_SLICE,
        "/dev",
        Some("devtmpfs"),
        MsFlags::empty(),
        NONE_OF_SLICE,
    )?;

    println!("Successfully mounted /dev");

    mount(
        NONE_OF_SLICE,
        "/proc",
        Some("proc"),
        MsFlags::empty(),
        NONE_OF_SLICE,
    )?;

    mount(
        NONE_OF_SLICE,
        "/sys",
        Some("sysfs"),
        MsFlags::empty(),
        NONE_OF_SLICE,
    )?;
    println!("Successfully mounted /sys");

    mount(
        NONE_OF_SLICE,
        "/run",
        Some("tmpfs"),
        MsFlags::MS_NOSUID | MsFlags::MS_NOEXEC,
        Some("size=81920k"),
    )?;
    println!("Successfully mounted /run");

    let mut my_subscriber_builder = FmtSubscriber::builder();

    let mut cmdline_data = String::new();
    let mut cmdline = File::open("/proc/cmdline")?;
    cmdline.read_to_string(&mut cmdline_data).expect("data should be well-formed and readable");

    let mut cmdline_key_values: Vec<(&str, Option<&str>)> = Default::default();

    for pair in cmdline_data.split(' ') {
        if pair.is_empty() {
            continue;
        }

        let mut kv_iter = pair.trim().splitn(2, '=');
        let key = kv_iter.next().expect("first element will exist, even if empty");
        let value = kv_iter.next();
        cmdline_key_values.push((key, value));
    }

    eprintln!("command line: {:?}", cmdline_key_values);

    let mut debug_level = 0;
    if get_cmdline_presence(&cmdline_key_values, "boot.trace") {
        eprintln!("enable trace");
        std::thread::sleep_ms(1500);
        debug_level = 2;
    }
    if get_cmdline_presence(&cmdline_key_values, "boot.debugtrace") {
        eprintln!("enable debugtrace");
        std::thread::sleep_ms(1500);
        debug_level = 3;
    }

    my_subscriber_builder = my_subscriber_builder.with_max_level(match debug_level {
        0 => TracingLevelFilter::ERROR,
        1 => TracingLevelFilter::WARN,
        2 => TracingLevelFilter::INFO,
        3 => TracingLevelFilter::DEBUG,
        _ => TracingLevelFilter::TRACE,
    });

    if let Some(root) = get_cmdline_value(&cmdline_key_values, "root") {
        let real_root_path = partition_def_to_path(root);
        event!(Level::DEBUG, "making /dev/root a symlink to {}", real_root_path.display());
        std::os::unix::fs::symlink(real_root_path, "/dev/root")?;
    }

    let mut version = String::new();
    let mut ver_fh = fs::File::open("/proc/version")?;
    ver_fh.read_to_string(&mut version)?;
    drop(ver_fh);

    println!("{}", version);

    let _udev = Command::new("/lib/systemd/systemd-udevd")
        .args(&["--daemon", "--resolve-names=never"])
        .spawn()?;

    Command::new("/usr/sbin/sysctl")
        .args(&[
            "-w",
            "net.ipv6.conf.all.autoconf=0",
            "net.ipv6.conf.all.accept_ra=1",
            "net.ipv6.conf.default.autoconf=0",
            "net.ipv6.conf.default.accept_ra=1",
        ])
        .spawn()?
        .wait()?;

    Command::new("/usr/bin/udevadm")
        .args(&["trigger", "--type=subsystems", "--action=add"])
        .spawn()?
        .wait()?;

    Command::new("/usr/bin/udevadm")
        .args(&["trigger", "--type=devices", "--action=add"])
        .spawn()?
        .wait()?;

    Command::new("/usr/bin/udevadm")
        .args(&["settle"])
        .spawn()?
        .wait()?;

    // Command::new("/usr/sbin/netman")
    //     // .args(&[])
    //     .spawn()?;
    // Command::new("/usr/bin/mount")
    //     // .args(&[])
    //     .spawn()
    //     .expect("failed to execute process");

    fs::create_dir("/_next-root")?;

    mount(
        NONE_OF_SLICE,
        "/_next-root",
        Some("tmpfs"),
        MsFlags::MS_NOSUID | MsFlags::MS_NODEV,
        Some("size=8388608k")
    )?;

    fs::create_dir("/_next-root/dev")?;
    fs::create_dir("/_next-root/images")?;
    fs::create_dir("/_next-root/nix")?;
    fs::create_dir("/_next-root/nix/store")?;
    fs::create_dir("/_next-root/proc")?;
    fs::create_dir("/_next-root/run")?;
    fs::create_dir("/_next-root/sys")?;
    fs::create_dir("/_next-root/usr")?;
    fs::create_dir("/_next-root/tmp")?;

    fs::create_dir("/_next-root/nix/.rw-store")?;
    fs::create_dir("/_next-root/nix/.ro-store")?;
    fs::create_dir("/_next-root/.old")?;

    // mount(
    //     NONE_OF_SLICE,
    //     "/_next-root/images",
    //     Some("tmpfs"),
    //     MsFlags::MS_NOSUID | MsFlags::MS_NODEV | MsFlags::MS_NOEXEC,
    //     Some("size=1048576k")
    // )?;

    mount(
        NONE_OF_SLICE,
        "/_next-root/tmp",
        Some("tmpfs"),
        MsFlags::MS_NOSUID | MsFlags::MS_NODEV | MsFlags::MS_NOEXEC,
        Some("size=1048576k")
    )?;

    // mount(
    //     NONE_OF_SLICE,
    //     "/_next-root/nix/.rw-store",
    //     Some("tmpfs"),
    //     MsFlags::MS_NOSUID | MsFlags::MS_NODEV,
    //     Some("size=8388608k")
    // )?;
    fs::create_dir("/_next-root/nix/.rw-store/store")?;
    fs::create_dir("/_next-root/nix/.rw-store/work")?;

    let modules = vec!["squashfs", "overlay", "ext4"];
    for module in modules {
        Command::new("/nix/store/bn6zidj988hc2pvp6k08slj5fk7qfcc9-kmod-27/bin/modprobe")
            .args(&[module])
            .spawn()
            .with_context(|| format!("failed to execute modprobe for {}", module))?
            .wait()
            .with_context(|| format!("modprobe failed for {}", module))?;
    }

    // {
    //     let mut input = File::open("/dev/disk/by-path/virtio-pci-0000:00:04.0").unwrap();
    //     let mut output = File::create("/_next-root/images/application-base.squashfs").unwrap();
    //     io::copy(&mut input, &mut output).unwrap();
    // }

    mount(
        Some("/dev/disk/by-path/virtio-pci-0000:00:04.0"),
        "/_next-root/nix/.ro-store",
        Some("squashfs"),
        MsFlags::MS_RDONLY,
        NONE_OF_SLICE,
    )?;

    mount(
        Some("/dev"),
        "/_next-root/dev",
        NONE_OF_SLICE,
        MsFlags::MS_BIND,
        NONE_OF_SLICE,
    )?;

    mount(
        Some("/sys"),
        "/_next-root/sys",
        NONE_OF_SLICE,
        MsFlags::MS_BIND,
        NONE_OF_SLICE,
    )?;

    mount(
        Some("/proc"),
        "/_next-root/proc",
        NONE_OF_SLICE,
        MsFlags::MS_BIND,
        NONE_OF_SLICE,
    )?;

    mount(
        Some("/run"),
        "/_next-root/run",
        NONE_OF_SLICE,
        MsFlags::MS_BIND,
        NONE_OF_SLICE,
    )?;

    visit_dirs("/_next-root/nix/.rw-store", 0)?;

    mount(
        NONE_OF_SLICE,
        "/_next-root/nix/store",
        Some("overlay"),
        MsFlags::empty(),
        Some("lowerdir=/_next-root/nix/.ro-store,upperdir=/_next-root/nix/.rw-store/store,workdir=/_next-root/nix/.rw-store/work")
    )?;

    for entry_res in fs::read_dir("/_next-root").unwrap() {
        let entry = entry_res?;

        println!("found /_next-root entry: {:?}", entry.path().display());
    }

    nix::unistd::chdir("/_next-root")?;

    mount(
        Some("/"),
        "/_next-root/.old",
        NONE_OF_SLICE,
        MsFlags::MS_BIND,
        NONE_OF_SLICE,
    )?;

    mount(
        Some("/_next-root"),
        "/",
        NONE_OF_SLICE,
        MsFlags::MS_MOVE,
        NONE_OF_SLICE,
    )?;

    nix::unistd::chroot(".")?;

    // Command::new("/nix/store/f7jzmxq9bpbxsg69cszx56mw14n115n5-bash-4.4-p23/bin/bash")
    //     .spawn()?
    //     .wait()?;

    if let Err(err) = Command::new("/nix/store/93l9n0msl71fw2ba3wbj9y5nx5mbzd8p-system-path/bin/find")
        .args(&["/.old", "-mount", "-depth", "-delete"])
        .spawn()?
        .wait()
    {
        eprintln!("error clearing out /.old: {}", err);
    }

    umount2("/.old", MntFlags::empty())?;

    use std::ffi::{CString, CStr};
    const ARRAY_OF_CSTR_EMPTY: &[&CStr] = &[];

    if let Some(vv) = get_cmdline_value(&cmdline_key_values, "nixos-system") {
        let vv = format!("{}/init", vv);
        let init = CString::new(vv).unwrap();
        if let Err(err) = nix::unistd::execve(&init, &[&init], &ARRAY_OF_CSTR_EMPTY) {
            eprintln!("error execing init: {:?}", err);
        }
    }

    sleep(Duration::new(5, 0));
    reboot(RebootMode::RB_POWER_OFF).expect("power off failed");
    Ok(())
}

// one possible implementation of walking a directory only visiting files
fn visit_dirs<P: AsRef<Path>>(dir: P, indent: usize) -> anyhow::Result<()> {
    let dir: &Path = dir.as_ref();
    let dir_disp = format!("{}", dir.display());
    if dir_disp.starts_with("/proc") {
        return Ok(());
    }

    if fs::metadata(&dir)
        .with_context(|| format!("error stating file {}", dir_disp))?
        .is_dir()
    {
        for entry in fs::read_dir(dir)? {
            let path = entry
                .with_context(|| format!("error stating file {}", dir.display()))?
                .path();

            let indentation: String = (0..indent).map(|_| ' ').collect();
            println!("{}{}", indentation, path.display());

            if fs::metadata(&path)
                .with_context(|| format!("error stating file {}", path.display()))?
                .is_dir()
            {
                visit_dirs(&path, indent + 2)?;
            }
        }
    }
    Ok(())
}
