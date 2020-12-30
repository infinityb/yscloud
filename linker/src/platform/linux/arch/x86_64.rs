pub const /*0002*/ SYSCALL_NR_OPEN: usize = 2;
pub const /*0004*/ SYSCALL_NR_STAT: usize = 4;
pub const /*0005*/ SYSCALL_NR_FSTAT: usize = 5;
pub const /*0006*/ SYSCALL_NR_LSTAT: usize = 6;
pub const /*0021*/ SYSCALL_NR_ACCESS: usize = 21;
pub const /*0041*/ SYSCALL_NR_SOCKET: usize = 41;
pub const /*0049*/ SYSCALL_NR_BIND: usize = 49;
pub const /*0054*/ SYSCALL_NR_SETSOCKOPT: usize = 54;
pub const /*0055*/ SYSCALL_NR_GETSOCKOPT: usize = 55;
pub const /*0059*/ SYSCALL_NR_EXECVE: usize = 59;
pub const /*0076*/ SYSCALL_NR_TRUNCATE: usize = 76;
pub const /*0082*/ SYSCALL_NR_RENAME: usize = 82;
pub const /*0085*/ SYSCALL_NR_CREAT: usize = 85;
pub const /*0086*/ SYSCALL_NR_LINK: usize = 86;
pub const /*0087*/ SYSCALL_NR_UNLINK: usize = 87;
pub const /*0090*/ SYSCALL_NR_CHMOD: usize = 90;
pub const /*0092*/ SYSCALL_NR_CHOWN: usize = 92;
pub const /*0133*/ SYSCALL_NR_MKNOD: usize = 133;
pub const /*0161*/ SYSCALL_NR_CHROOT: usize = 161;
pub const /*0257*/ SYSCALL_NR_OPENAT: usize = 257;
pub const /*0259*/ SYSCALL_NR_MKNODAT: usize = 259;
pub const /*0303*/ SYSCALL_NR_NAME_TO_HANDLE_AT: usize = 303;
pub const /*0304*/ SYSCALL_NR_OPEN_BY_HANDLE_AT: usize = 304;
pub const /*0322*/ SYSCALL_NR_EXECVEAT: usize = 322;

pub const SYSCALL_SET_FILESYSTEM: &[usize] = &[
    /*0002*/ SYSCALL_NR_OPEN,
    /*0004*/ SYSCALL_NR_STAT,
    /*0005*/ SYSCALL_NR_FSTAT,
    /*0006*/ SYSCALL_NR_LSTAT,
    /*0021*/ SYSCALL_NR_ACCESS,
    /*0059*/ SYSCALL_NR_EXECVE,
    /*0082*/ SYSCALL_NR_RENAME,
    /*0085*/ SYSCALL_NR_CREAT,
    /*0086*/ SYSCALL_NR_LINK,
    /*0087*/ SYSCALL_NR_UNLINK,
    /*0090*/ SYSCALL_NR_CHMOD,
    /*0092*/ SYSCALL_NR_CHOWN,
    /*0133*/ SYSCALL_NR_MKNOD,
    /*0161*/ SYSCALL_NR_CHROOT,
    /*0076*/ SYSCALL_NR_TRUNCATE,
];

pub const SYSCALL_SET_LEGACY: &[usize] = &[
    /*0303*/ SYSCALL_NR_NAME_TO_HANDLE_AT,
    /*0304*/ SYSCALL_NR_OPEN_BY_HANDLE_AT,
];

pub const SYSCALL_SET_NETWORK: &[usize] = &[
    /*0041*/ SYSCALL_NR_SOCKET,
    /*0049*/ SYSCALL_NR_BIND,
    /*0054*/ SYSCALL_NR_SETSOCKOPT,
];

// management related functions, maybe immutable?
pub const SYSCALL_SET_NETWORK_LOW: &[usize] = &[
    /*0055*/ SYSCALL_NR_GETSOCKOPT,
];
