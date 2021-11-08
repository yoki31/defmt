use std::{process::Command, str::FromStr, sync::Mutex};

use anyhow::{anyhow, Context};
use colored::Colorize;
use once_cell::sync::Lazy;
use similar::{ChangeTag, TextDiff};
use structopt::StructOpt;

mod backcompat;
mod targets;
mod utils;

use crate::utils::{
    load_expected_output, overwrite_expected_output, run_capturing_stdout, rustc_is_nightly, Cmd,
};

static ALL_ERRORS: Lazy<Mutex<Vec<String>>> = Lazy::new(|| Mutex::new(vec![]));

const SNAPSHOT_TESTS_DIRECTORY: &str = "firmware/qemu";
const ALL_SNAPSHOT_TESTS: [&str; 12] = [
    "log",
    "bitflags",
    "timestamp",
    "panic",
    "assert",
    "assert-eq",
    "assert-ne",
    "unwrap",
    "defmt-test",
    "hints",
    "hints_inner",
    "dbg",
];

#[derive(Debug)]
struct Snapshot(String);

impl Snapshot {
    pub fn name(&self) -> &str {
        &self.0
    }
}

impl FromStr for Snapshot {
    type Err = String;

    fn from_str(test: &str) -> Result<Self, Self::Err> {
        if ALL_SNAPSHOT_TESTS.contains(&test) {
            Ok(Self(String::from(test)))
        } else {
            Err(format!(
                "Specified test '{}' does not exist, available tests are: {:?}",
                test, ALL_SNAPSHOT_TESTS
            ))
        }
    }
}

#[derive(Debug, StructOpt)]
struct Options {
    #[structopt(subcommand)]
    cmd: TestCommand,
    /// Treat compiler warnings as errors (`RUSTFLAGS="--deny warnings"`)
    #[structopt(long, short)]
    deny_warnings: bool,
    /// Keep target toolchains that were installed as dependency
    #[structopt(long, short)]
    keep_targets: bool,
}

#[derive(Debug, StructOpt)]
#[allow(clippy::enum_variant_names)]
enum TestCommand {
    TestAll,
    TestBackcompat,
    TestBook,
    TestCross,
    TestHost,
    TestLint,
    /// Run snapshot tests or optionally overwrite the expected output
    TestSnapshot {
        /// Overwrite the expected output instead of comparing it.
        #[structopt(long)]
        overwrite: bool,
        /// Runs a single snapshot test in Debug mode
        #[structopt()]
        single: Option<Snapshot>,
    },
}

fn main() -> anyhow::Result<()> {
    let opt: Options = Options::from_args();
    let mut added_targets = None;

    match opt.cmd {
        TestCommand::TestBook => test_book(),
        TestCommand::TestBackcompat => backcompat::test(),
        TestCommand::TestHost => test_host(opt.deny_warnings),
        TestCommand::TestLint => test_lint(),

        // following tests need to install additional targets
        cmd => {
            added_targets = Some(targets::install().expect("Error while installing required targets"));
            match cmd {
                TestCommand::TestCross => test_cross(),
                TestCommand::TestSnapshot { overwrite, single } => {
                    test_snapshot(overwrite, single);
                }
                TestCommand::TestAll => {
                    test_host(opt.deny_warnings);
                    test_cross();
                    test_snapshot(false, None);
                    backcompat::test();
                    test_book();
                    test_lint();
                }
                _ => unreachable!("get handled in outer `match`"),
            }
        }
    }

    if let Some(added_targets) = added_targets {
        if !opt.keep_targets && !added_targets.is_empty() {
            targets::uninstall(added_targets)
        }
    }

    let all_errors = ALL_ERRORS.lock().unwrap();
    if !all_errors.is_empty() {
        eprintln!();
        Err(anyhow!("😔 some tests failed: {:#?}", all_errors))
    } else {
        Ok(())
    }
}

fn do_test(test: impl FnOnce() -> anyhow::Result<()>, context: &str) {
    test().unwrap_or_else(|e| ALL_ERRORS.lock().unwrap().push(format!("{}: {}", context, e)));
}

fn test_host(deny_warnings: bool) {
    println!("🧪 host");

    let env = if deny_warnings {
        vec![("RUSTFLAGS", "--deny warnings")]
    } else {
        vec![]
    };

    [
        Cmd::new("cargo check --workspace").envs(&env),
        Cmd::new("cargo check --workspace --features unstable-test").envs(&env),
        Cmd::new("cargo check --workspace --features alloc").envs(&env),
        Cmd::new("cargo test --workspace --features unstable-test"),
    ]
    .into_iter()
    .for_each(|cmd| do_test(|| cmd.run(), "host"));
}

fn test_cross() {
    println!("🧪 cross");
    let targets = [
        "thumbv6m-none-eabi",
        "thumbv8m.base-none-eabi",
        "riscv32i-unknown-none-elf",
    ];

    for target in &targets {
        [
            Cmd::new("cargo check -p defmt").target(target),
            Cmd::new("cargo check -p defmt --features alloc").target(target),
        ]
        .into_iter()
        .for_each(|cmd| do_test(|| cmd.run(), "cross"));
    }

    [
        Cmd::new("cargo check --target thumbv6m-none-eabi --exclude defmt-itm --exclude firmware")
            .cwd("firmware"),
        Cmd::new("cargo check --target thumbv7em-none-eabi").cwd("firmware"),
        Cmd::new("cargo check --target thumbv6m-none-eabi --features print-defmt")
            .cwd("firmware/panic-probe"),
        Cmd::new("cargo check --target thumbv6m-none-eabi --features print-rtt")
            .cwd("firmware/panic-probe"),
    ]
    .into_iter()
    .for_each(|cmd| do_test(|| cmd.run(), "cross"));
}

fn test_snapshot(overwrite: bool, snapshot: Option<Snapshot>) {
    println!("🧪 qemu/snapshot");

    match snapshot {
        None => test_all_snapshots(overwrite),
        Some(snapshot) => {
            do_test(
                || test_single_snapshot(snapshot.name(), "", overwrite),
                "qemu/snapshot",
            );
        }
    }
}

fn test_all_snapshots(overwrite: bool) {
    let mut tests = ALL_SNAPSHOT_TESTS.to_vec();

    if rustc_is_nightly() {
        tests.push("alloc");
    }

    for test in tests {
        let features = if test == "alloc" { "alloc" } else { "" };

        do_test(
            || test_single_snapshot(test, features, overwrite),
            "qemu/snapshot",
        );
    }
}

fn test_single_snapshot(name: &str, features: &str, overwrite: bool) -> anyhow::Result<()> {
    println!("{}", name.bold());

    let is_test = name.contains("test");

    let mut args = if is_test {
        vec!["-q", "tt", name]
    } else {
        vec!["-q", "rb", name]
    };

    if !features.is_empty() {
        args.extend_from_slice(&["--features", features]);
    }

    let actual = run_capturing_stdout(
        Command::new("cargo")
            .args(&args)
            .env("DEFMT_LOG", "trace")
            .current_dir(SNAPSHOT_TESTS_DIRECTORY),
    )
    .with_context(|| name.to_string())?;

    if overwrite {
        overwrite_expected_output(name, actual.as_bytes(), is_test)?;
        return Ok(());
    }

    let expected = load_expected_output(name, is_test)?;
    let diff = TextDiff::from_lines(&expected, &actual);

    // if anything isn't ChangeTag::Equal, print it and turn on error flag
    let mut actual_matches_expected = true;
    for op in diff.ops() {
        for change in diff.iter_changes(op) {
            let styled_change = match change.tag() {
                ChangeTag::Delete => Some(("-".bold().red(), change.to_string().red())),
                ChangeTag::Insert => Some(("+".bold().green(), change.to_string().green())),
                ChangeTag::Equal => None,
            };
            if let Some((sign, change)) = styled_change {
                actual_matches_expected = false;
                eprint!("{}{}", sign, change);
            }
        }
    }

    if actual_matches_expected {
        Ok(())
    } else {
        Err(anyhow!("{}", name))
    }
}

fn test_book() {
    println!("🧪 book");

    [
        Cmd::new("cargo clean"),
        Cmd::new("cargo build -p defmt --features unstable-test"),
        Cmd::new("mdbook test -L ../target/debug -L ../target/debug/deps")
            .cwd("book")
            // logging macros need this but mdbook, not being cargo, doesn't set it so we use a dummy value
            .envs(&[("CARGO_CRATE_NAME", "krate")]),
    ]
    .into_iter()
    .for_each(|cmd| do_test(|| cmd.run(), "book"));
}

fn test_lint() {
    println!("🧪 lint");
    do_test(|| Cmd::new("cargo clean").run(), "lint");
    do_test(|| Cmd::new("cargo fmt --all -- --check").run(), "lint");
    do_test(|| Cmd::new("cargo clippy --workspace").run(), "lint");
}
