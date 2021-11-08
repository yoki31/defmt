mod cmd;

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    str,
};

use anyhow::{anyhow, Context};
use colored::Colorize;

pub use cmd::Cmd;

pub fn load_expected_output(name: &str, is_test: bool) -> anyhow::Result<String> {
    let path = expected_output_path(name, is_test);

    fs::read_to_string(&path).with_context(|| {
        format!(
            "Failed to load expected output data from {}",
            path.to_str().unwrap_or("(non-Unicode path)")
        )
    })
}

pub fn overwrite_expected_output(name: &str, contents: &[u8], is_test: bool) -> anyhow::Result<()> {
    let file = expected_output_path(name, is_test);
    let path = Path::new(&file);

    fs::write(path, contents).with_context(|| {
        format!(
            "Failed to overwrite expected output data to {}",
            path.to_str().unwrap_or("(non-Unicode path)")
        )
    })
}

fn expected_output_path(name: &str, is_test: bool) -> PathBuf {
    const PROJECT_DIR: &str = "firmware/qemu";

    let mut path = PathBuf::from(PROJECT_DIR);
    if is_test {
        path.push("tests")
    } else {
        path.push("src");
        path.push("bin");
    };
    path.push(name);
    path.set_extension("out");
    path
}

/// Execute the [`Command`]. If success return `stdout`, if failure print to `stderr`
pub fn run_capturing_stdout(cmd: &mut Command) -> anyhow::Result<String> {
    let output = cmd.output()?;
    match output.status.success() {
        true => Ok(str::from_utf8(&output.stdout)?.to_string()),
        false => {
            eprintln!("{}", str::from_utf8(&output.stderr)?.dimmed());
            Err(anyhow!(""))
        }
    }
}

pub fn rustc_is_nightly() -> bool {
    // if this crashes the system is not in a good state, so we'll not pretend to be able to recover
    let out = run_capturing_stdout(Command::new("rustc").args(&["-V"])).unwrap();
    out.contains("nightly")
}
