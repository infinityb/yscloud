use std::collections::HashSet;

use seccomp::{Context, Op, Action, Rule, Compare, SeccompError};

use super::arch::current as arch;

const NAMED_GROUPS: &[(&str, &[usize])] = &[
    ("@filesystem", arch::SYSCALL_SET_FILESYSTEM),
    ("@network", arch::SYSCALL_SET_NETWORK),
    ("@network-low", arch::SYSCALL_SET_NETWORK_LOW),
];

pub fn setup(allow: &HashSet<&'static str>) -> Result<(), SeccompError> {
    let mut ctx = Context::default(Action::Allow)?;

    if allow.contains("*") {
        return Ok(());
    }

    let mut allow_set = HashSet::<usize>::new();
    for (name, syscalls) in NAMED_GROUPS {
        if allow.contains(name) {
            for syscall in syscalls.iter() {
                allow_set.insert(*syscall);
            }
        }
    }

    // for syscall in arch::SYSCALL_SET_LEGACY {
    //     ctx.add_rule(Rule::new(*syscall,
    //         Compare::arg(0)
    //                 .with(0)
    //                 .using(Op::Ge)
    //                 .build().unwrap(),
    //         Action::KillProcess,
    //     ))?;
    // }

    for (_, syscalls) in NAMED_GROUPS {
        for syscall in syscalls.iter() {
            if !allow_set.contains(syscall) {
                ctx.add_rule(Rule::new(*syscall,
                    Compare::arg(0)
                            .with(0)
                            .using(Op::Ge)
                            .build().unwrap(),
                    Action::Errno(libc::EPERM),
                ))?;
            }
        }
    }

    ctx.load()
}