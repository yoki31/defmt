use std::process::Command;

use anyhow::anyhow;

pub struct Cmd {
    cmd_line: String,
    cmd: Command,
}

impl Cmd {
    pub fn new(cmd_line: &str) -> Self {
        let mut cmd_split = cmd_line.split(' ');
        let mut cmd = Command::new(cmd_split.nth(0).unwrap());
        cmd.args(cmd_split);

        Self {
            cmd_line: cmd_line.to_string(),
            cmd,
        }
    }

    pub fn cwd(&mut self, cwd: &str) -> &mut Self {
        self.cmd_line = format!("{}$ {}", cwd, self.cmd_line);
        self.cmd.current_dir(cwd);
        self
    }

    pub fn envs(&mut self, envs: &[(&str, &str)]) -> &mut Self {
        self.cmd.envs(envs.iter().cloned());
        self
    }

    pub fn target(&mut self, target: &str) -> &mut Self {
        self.cmd.args(&["--target", target]);
        self
    }

    pub fn run(&mut self) -> anyhow::Result<()> {
        let cmd_line = self.cmd_line.as_str();
        println!("🏃 {}", cmd_line);

        self.cmd
            .status()
            .map_err(|e| anyhow!("could not run '{}': {}", cmd_line, e))
            .and_then(|e| match e.success() {
                true => Ok(()),
                false => Err(anyhow!("'{}' did not finish successfully: {}", cmd_line, e)),
            })
    }
}
