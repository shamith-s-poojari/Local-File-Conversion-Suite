//! A hand-written seccomp filter for bubblewrap's `--seccomp FD` (SEC-5).
//!
//! The syscalls here are ones a converter never needs and that widen what a
//! compromised worker can reach: tracing other processes, kernel modules,
//! mounts, namespaces, BPF, keyrings, clocks. They are denied with `EPERM`
//! (not killed: a library probing for a feature then falls back cleanly);
//! everything else is allowed, so this is a hardening layer on top of the
//! namespaces, not the sandbox itself.
//!
//! The program is classic BPF in the layout bubblewrap reads: a bare array
//! of `struct sock_filter` (`u16 code, u8 jt, u8 jf, u32 k`, native endian),
//! not a `sock_fprog`. Pure byte generation, so it compiles and is tested on
//! every OS; only Linux uses it.
//!
//! bubblewrap applies the filter last, after it has made its own namespaces
//! and mounts, so denying `unshare` and `mount` doesn't stop it starting.
//!
//! **Architectures.** The filter is built for the machine's own architecture
//! (x86-64 or AArch64). Syscall numbers differ per architecture, so a call
//! made through another ABI (32-bit `int 0x80` on an x86-64 kernel, the x32
//! ABI) would slip past a number list: those are denied outright, since no
//! worker runs 32-bit code. On any other architecture there is no table and
//! [`host_arch`] is `None`: no filter is applied (the sandbox is then the
//! namespaces alone) and the Linux self-test's ptrace check, which is only
//! compiled for the two known architectures, doesn't cover it.

#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Arch {
    X86_64,
    Aarch64,
}

impl Arch {
    /// `AUDIT_ARCH_*` from `<linux/audit.h>`: EM_* | 64BIT | LE.
    const fn audit(self) -> u32 {
        match self {
            Arch::X86_64 => 0xC000_003E,
            Arch::Aarch64 => 0xC000_00B7,
        }
    }
}

/// The architecture this binary runs on, if the filter knows it.
pub(crate) fn host_arch() -> Option<Arch> {
    if cfg!(target_arch = "x86_64") {
        Some(Arch::X86_64)
    } else if cfg!(target_arch = "aarch64") {
        Some(Arch::Aarch64)
    } else {
        None
    }
}

/// Denied syscalls as (name, x86-64 number, AArch64 number). `None`: the
/// architecture has no such call (AArch64 never had `iopl`/`ioperm`).
/// Numbers are from the kernel's tables: `arch/x86/entry/syscalls/
/// syscall_64.tbl` and `include/uapi/asm-generic/unistd.h` (AArch64); a
/// Linux-only test compares the host's column with the `libc` crate.
/// `personality` is left out: glibc and some libraries call it legitimately.
const DENIED: &[(&str, Option<u32>, Option<u32>)] = &[
    ("ptrace", Some(101), Some(117)),
    ("process_vm_readv", Some(310), Some(270)),
    ("process_vm_writev", Some(311), Some(271)),
    ("keyctl", Some(250), Some(219)),
    ("add_key", Some(248), Some(217)),
    ("request_key", Some(249), Some(218)),
    ("bpf", Some(321), Some(280)),
    ("perf_event_open", Some(298), Some(241)),
    ("userfaultfd", Some(323), Some(282)),
    ("kexec_load", Some(246), Some(104)),
    ("kexec_file_load", Some(320), Some(294)),
    ("init_module", Some(175), Some(105)),
    ("finit_module", Some(313), Some(273)),
    ("delete_module", Some(176), Some(106)),
    ("mount", Some(165), Some(40)),
    ("umount2", Some(166), Some(39)),
    ("pivot_root", Some(155), Some(41)),
    // The new mount API (same numbers on every architecture).
    ("open_tree", Some(428), Some(428)),
    ("move_mount", Some(429), Some(429)),
    ("fsopen", Some(430), Some(430)),
    ("fsconfig", Some(431), Some(431)),
    ("fsmount", Some(432), Some(432)),
    ("fspick", Some(433), Some(433)),
    ("mount_setattr", Some(442), Some(442)),
    ("swapon", Some(167), Some(224)),
    ("swapoff", Some(168), Some(225)),
    ("reboot", Some(169), Some(142)),
    ("setns", Some(308), Some(268)),
    ("unshare", Some(272), Some(97)),
    ("open_by_handle_at", Some(304), Some(265)),
    ("name_to_handle_at", Some(303), Some(264)),
    ("iopl", Some(172), None),
    ("ioperm", Some(173), None),
    ("acct", Some(163), Some(89)),
    ("quotactl", Some(179), Some(60)),
    ("lookup_dcookie", Some(212), Some(18)),
    ("settimeofday", Some(164), Some(170)),
    ("clock_settime", Some(227), Some(112)),
    ("clock_adjtime", Some(305), Some(266)),
    ("adjtimex", Some(159), Some(171)),
    ("syslog", Some(103), Some(116)),
];

// Classic BPF opcodes (<linux/bpf_common.h>) and seccomp constants.
const LD_W_ABS: u16 = 0x20;
const JEQ_K: u16 = 0x15;
const JSET_K: u16 = 0x45;
const RET_K: u16 = 0x06;
const SECCOMP_RET_ALLOW: u32 = 0x7fff_0000;
const SECCOMP_RET_ERRNO: u32 = 0x0005_0000;
const EPERM: u32 = 1;
/// Offsets in `struct seccomp_data`.
const OFFSET_NR: u32 = 0;
const OFFSET_ARCH: u32 = 4;
/// x86-64 marks x32-ABI calls with this bit in the number.
const X32_BIT: u32 = 0x4000_0000;

#[derive(Clone, Copy)]
struct Insn {
    code: u16,
    jt: u8,
    jf: u8,
    k: u32,
}

/// The program for `arch`, ready to write to bubblewrap's descriptor.
pub(crate) fn program(arch: Arch) -> Vec<u8> {
    let numbers: Vec<u32> = DENIED
        .iter()
        .filter_map(|(_, x86, arm)| match arch {
            Arch::X86_64 => *x86,
            Arch::Aarch64 => *arm,
        })
        .collect();
    let x32 = arch == Arch::X86_64;
    // Fixed head: load arch, check it, load nr, (x32 test).
    let head = 3 + usize::from(x32);
    let allow = head + numbers.len();
    let deny = allow + 1;
    // Jump offsets count from the next instruction and fit in a byte: the
    // list is short, and this fails loudly if someone makes it long.
    let to_deny = |at: usize| u8::try_from(deny - at - 1).expect("deny list too long for cBPF");

    let mut p = vec![
        Insn {
            code: LD_W_ABS,
            jt: 0,
            jf: 0,
            k: OFFSET_ARCH,
        },
        // Another ABI than the table's: deny, don't guess.
        Insn {
            code: JEQ_K,
            jt: 0,
            jf: to_deny(1),
            k: arch.audit(),
        },
        Insn {
            code: LD_W_ABS,
            jt: 0,
            jf: 0,
            k: OFFSET_NR,
        },
    ];
    if x32 {
        p.push(Insn {
            code: JSET_K,
            jt: to_deny(3),
            jf: 0,
            k: X32_BIT,
        });
    }
    for (i, nr) in numbers.iter().enumerate() {
        p.push(Insn {
            code: JEQ_K,
            jt: to_deny(head + i),
            jf: 0,
            k: *nr,
        });
    }
    p.push(Insn {
        code: RET_K,
        jt: 0,
        jf: 0,
        k: SECCOMP_RET_ALLOW,
    });
    p.push(Insn {
        code: RET_K,
        jt: 0,
        jf: 0,
        k: SECCOMP_RET_ERRNO | EPERM,
    });

    let mut bytes = Vec::with_capacity(p.len() * 8);
    for i in p {
        bytes.extend_from_slice(&i.code.to_ne_bytes());
        bytes.push(i.jt);
        bytes.push(i.jf);
        bytes.extend_from_slice(&i.k.to_ne_bytes());
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    const DENY: u32 = SECCOMP_RET_ERRNO | EPERM;

    fn insns(bytes: &[u8]) -> Vec<(u16, u8, u8, u32)> {
        assert_eq!(bytes.len() % 8, 0);
        bytes
            .chunks(8)
            .map(|c| {
                (
                    u16::from_ne_bytes([c[0], c[1]]),
                    c[2],
                    c[3],
                    u32::from_ne_bytes([c[4], c[5], c[6], c[7]]),
                )
            })
            .collect()
    }

    /// Just enough of a cBPF machine to run these programs.
    fn run(bytes: &[u8], arch: u32, nr: u32) -> u32 {
        let prog = insns(bytes);
        let (mut acc, mut pc) = (0u32, 0usize);
        loop {
            let (code, jt, jf, k) = prog[pc];
            pc += 1;
            match code {
                LD_W_ABS => {
                    acc = match k {
                        OFFSET_NR => nr,
                        OFFSET_ARCH => arch,
                        _ => panic!("unexpected load"),
                    }
                }
                JEQ_K => pc += usize::from(if acc == k { jt } else { jf }),
                JSET_K => pc += usize::from(if acc & k != 0 { jt } else { jf }),
                RET_K => return k,
                _ => panic!("unexpected opcode {code:#x}"),
            }
        }
    }

    #[test]
    fn checks_the_architecture_first() {
        for arch in [Arch::X86_64, Arch::Aarch64] {
            let p = insns(&program(arch));
            assert_eq!(p[0], (LD_W_ABS, 0, 0, OFFSET_ARCH));
            assert_eq!((p[1].0, p[1].3), (JEQ_K, arch.audit()));
            // The last two are the verdicts.
            assert_eq!(p[p.len() - 2].3, SECCOMP_RET_ALLOW);
            assert_eq!(p[p.len() - 1].3, DENY);
        }
    }

    #[test]
    fn the_program_has_the_expected_size() {
        let n = DENIED.len();
        // 3 head + x32 test + one per call + 2 verdicts; AArch64 lacks 2 calls.
        assert_eq!(program(Arch::X86_64).len(), 8 * (3 + 1 + n + 2));
        assert_eq!(program(Arch::Aarch64).len(), 8 * (3 + (n - 2) + 2));
    }

    #[test]
    fn every_listed_syscall_is_refused_with_eperm() {
        for (arch, pick) in [(Arch::X86_64, 0), (Arch::Aarch64, 1)] {
            let prog = program(arch);
            for (name, x86, arm) in DENIED {
                let Some(nr) = (if pick == 0 { x86 } else { arm }) else {
                    continue;
                };
                assert_eq!(run(&prog, arch.audit(), *nr), DENY, "{arch:?} {name}");
            }
        }
    }

    #[test]
    fn everything_else_is_allowed() {
        for (arch, reads) in [
            (Arch::X86_64, [0, 1, 2, 9, 60]),
            (Arch::Aarch64, [63, 64, 214, 222, 93]),
        ] {
            let prog = program(arch);
            for nr in reads {
                assert_eq!(
                    run(&prog, arch.audit(), nr),
                    SECCOMP_RET_ALLOW,
                    "{arch:?} {nr}"
                );
            }
        }
        // x86-64's 101 is ptrace, but AArch64's 101 is not on the list.
        assert_eq!(
            run(&program(Arch::Aarch64), Arch::Aarch64.audit(), 101),
            SECCOMP_RET_ALLOW
        );
    }

    #[test]
    fn other_abis_are_denied_not_matched_by_number() {
        let x86 = program(Arch::X86_64);
        // AUDIT_ARCH_I386, and the x32 flag on an otherwise harmless call.
        assert_eq!(run(&x86, 0x4000_0003, 1), DENY);
        assert_eq!(run(&x86, Arch::X86_64.audit(), X32_BIT | 1), DENY);
        assert_eq!(run(&x86, Arch::X86_64.audit(), X32_BIT | 101), DENY);
        // AArch64 filter, AUDIT_ARCH_ARM call: also refused.
        assert_eq!(run(&program(Arch::Aarch64), 0x4000_0028, 1), DENY);
    }

    #[test]
    fn numbers_are_unique_per_architecture() {
        for pick in 0..2 {
            let mut seen = std::collections::BTreeSet::new();
            for (name, x86, arm) in DENIED {
                if let Some(nr) = if pick == 0 { x86 } else { arm } {
                    assert!(seen.insert(*nr), "{name} repeats {nr}");
                }
            }
        }
    }

    /// The host's column must match what libc says the kernel numbers are.
    #[cfg(all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ))]
    #[test]
    fn the_host_numbers_match_libc() {
        let libc_nr: &[(&str, libc::c_long)] = &[
            ("ptrace", libc::SYS_ptrace),
            ("process_vm_readv", libc::SYS_process_vm_readv),
            ("process_vm_writev", libc::SYS_process_vm_writev),
            ("keyctl", libc::SYS_keyctl),
            ("add_key", libc::SYS_add_key),
            ("request_key", libc::SYS_request_key),
            ("bpf", libc::SYS_bpf),
            ("perf_event_open", libc::SYS_perf_event_open),
            ("userfaultfd", libc::SYS_userfaultfd),
            ("kexec_load", libc::SYS_kexec_load),
            ("kexec_file_load", libc::SYS_kexec_file_load),
            ("init_module", libc::SYS_init_module),
            ("finit_module", libc::SYS_finit_module),
            ("delete_module", libc::SYS_delete_module),
            ("mount", libc::SYS_mount),
            ("umount2", libc::SYS_umount2),
            ("pivot_root", libc::SYS_pivot_root),
            ("open_tree", libc::SYS_open_tree),
            ("move_mount", libc::SYS_move_mount),
            ("fsopen", libc::SYS_fsopen),
            ("fsconfig", libc::SYS_fsconfig),
            ("fsmount", libc::SYS_fsmount),
            ("fspick", libc::SYS_fspick),
            ("mount_setattr", libc::SYS_mount_setattr),
            ("swapon", libc::SYS_swapon),
            ("swapoff", libc::SYS_swapoff),
            ("reboot", libc::SYS_reboot),
            ("setns", libc::SYS_setns),
            ("unshare", libc::SYS_unshare),
            ("open_by_handle_at", libc::SYS_open_by_handle_at),
            ("name_to_handle_at", libc::SYS_name_to_handle_at),
            #[cfg(target_arch = "x86_64")]
            ("iopl", libc::SYS_iopl),
            #[cfg(target_arch = "x86_64")]
            ("ioperm", libc::SYS_ioperm),
            ("acct", libc::SYS_acct),
            ("quotactl", libc::SYS_quotactl),
            ("lookup_dcookie", libc::SYS_lookup_dcookie),
            ("settimeofday", libc::SYS_settimeofday),
            ("clock_settime", libc::SYS_clock_settime),
            ("clock_adjtime", libc::SYS_clock_adjtime),
            ("adjtimex", libc::SYS_adjtimex),
            ("syslog", libc::SYS_syslog),
        ];
        let arch = host_arch().unwrap();
        for (name, x86, arm) in DENIED {
            let ours = if arch == Arch::X86_64 { x86 } else { arm };
            let theirs = libc_nr.iter().find(|(n, _)| n == name).map(|(_, v)| *v);
            assert_eq!(ours.map(i64::from), theirs, "{name}");
        }
    }
}
