use nix::unistd::{fork, ForkResult, execvp, pipe, dup2, close, chdir, Pid};
use nix::sys::wait::{waitpid, WaitStatus};
use nix::fcntl::{open, OFlag};
use nix::sys::stat::Mode;
use std::env;
use std::ffi::CString;
use std::io::{self, Write};
use std::os::unix::io::{AsRawFd, OwnedFd};
use std::path::Path;
use std::process::exit;

fn main() {
    loop {
        let cwd = env::current_dir().unwrap();
        print!("{} $ ", cwd.display());
        io::stdout().flush().unwrap();

        let mut input = String::new();
        io::stdin().read_line(&mut input).unwrap();
        let input = input.trim();

        if input.is_empty() {
            continue;
        } else if input == "exit" {
            break;
        } else if input.starts_with("cd ") {
            let dir = input[3..].trim();
            if let Err(e) = chdir(Path::new(dir)) {
                eprintln!("cd error: {}", e);
            }
            continue;
        }

        run_command(input);
    }
}

fn run_command(input: &str) {
    let background = input.ends_with("&");
    let input = input.trim_end_matches('&').trim();

    let pipeline_parts: Vec<&str> = input.split('|').map(|s| s.trim()).collect();
    if pipeline_parts.len() > 1 {
        run_pipeline(pipeline_parts, background);
        return;
    }

    let (cmd, infile, outfile) = parse_redirection(input);

    match unsafe { fork() } {
        Ok(ForkResult::Child) => {
            if let Some(file) = infile {
                let fd = open(file.as_str(), OFlag::O_RDONLY, Mode::empty()).unwrap_or_else(|e| {
                    eprintln!("Input file error: {}", e);
                    exit(1);
                });
                dup2(fd, 0).unwrap_or_else(|e| {
                    eprintln!("dup2 failed: {}", e);
                    exit(1);
                });
                close(fd).ok();
            }
            if let Some(file) = outfile {
                let fd = open(file.as_str(), OFlag::O_CREAT | OFlag::O_WRONLY | OFlag::O_TRUNC, Mode::from_bits_truncate(0o644)).unwrap_or_else(|e| {
                    eprintln!("Output file error: {}", e);
                    exit(1);
                });
                dup2(fd, 1).unwrap_or_else(|e| {
                    eprintln!("dup2 failed: {}", e);
                    exit(1);
                });
                close(fd).ok();
            }
            exec_command(&cmd);
        }
        Ok(ForkResult::Parent { child }) => {
            if background {
                println!("Started background process with PID {}", child);
            } else {
                match waitpid(child, None) {
                    Ok(WaitStatus::Exited(_, status)) => {
                        if status != 0 {
                            eprintln!("Process exited with status {}", status);
                        }
                    }
                    Ok(_) => {}
                    Err(e) => eprintln!("waitpid error: {}", e),
                }
            }
        }
        Err(e) => eprintln!("Fork failed: {}", e),
    }
}

fn exec_command(cmd_parts: &[String]) {
    if cmd_parts.is_empty() {
        exit(0);
    }
    let cstrs: Vec<CString> = cmd_parts.iter().map(|s| CString::new(s.as_str()).unwrap()).collect();
    let _ = execvp(&cstrs[0], &cstrs);
    eprintln!("Command not found: {}", cmd_parts[0]);
    exit(1);
}

fn parse_redirection(input: &str) -> (Vec<String>, Option<String>, Option<String>) {
    let mut tokens: Vec<&str> = input.split_whitespace().collect();
    let mut infile = None;
    let mut outfile = None;
    let mut i = 0;

    while i < tokens.len() {
        match tokens[i] {
            "<" => {
                infile = Some(tokens.remove(i + 1).to_string());
                tokens.remove(i);
            }
            ">" => {
                outfile = Some(tokens.remove(i + 1).to_string());
                tokens.remove(i);
            }
            _ => i += 1,
        }
    }

    let cmd = tokens.into_iter().map(|s| s.to_string()).collect();
    (cmd, infile, outfile)
}

fn run_pipeline(commands: Vec<&str>, background: bool) {
    let mut fds: Vec<(OwnedFd, OwnedFd)> = Vec::new();

    for _ in 0..commands.len() - 1 {
        let (read_fd, write_fd) = pipe().unwrap();
        fds.push((read_fd, write_fd));
    }

    for (i, cmd_str) in commands.iter().enumerate() {
        let (cmd, infile, outfile) = parse_redirection(cmd_str);

        match unsafe { fork() } {
            Ok(ForkResult::Child) => {
                if i > 0 {
                    dup2(fds[i - 1].0.as_raw_fd(), 0).unwrap_or_else(|e| {
                        eprintln!("dup2 failed: {}", e);
                        exit(1);
                    });
                }
                if i < fds.len() {
                    dup2(fds[i].1.as_raw_fd(), 1).unwrap_or_else(|e| {
                        eprintln!("dup2 failed: {}", e);
                        exit(1);
                    });
                }

                // Close pipes after usage in the child process to prevent issues
                for (r, w) in &fds {
                    close(r.as_raw_fd()).ok();
                    close(w.as_raw_fd()).ok();
                }

                if i == 0 {
                    if let Some(ref file) = infile {
                        let fd = open(file.as_str(), OFlag::O_RDONLY, Mode::empty()).unwrap_or_else(|e| {
                            eprintln!("Input file error: {}", e);
                            exit(1);
                        });
                        dup2(fd, 0).unwrap_or_else(|e| {
                            eprintln!("dup2 failed: {}", e);
                            exit(1);
                        });
                        close(fd).ok();
                    }
                }

                if i == commands.len() - 1 {
                    if let Some(ref file) = outfile {
                        let fd = open(file.as_str(), OFlag::O_CREAT | OFlag::O_WRONLY | OFlag::O_TRUNC, Mode::from_bits_truncate(0o644)).unwrap_or_else(|e| {
                            eprintln!("Output file error: {}", e);
                            exit(1);
                        });
                        dup2(fd, 1).unwrap_or_else(|e| {
                            eprintln!("dup2 failed: {}", e);
                            exit(1);
                        });
                        close(fd).ok();
                    }
                }

                exec_command(&cmd);
            }
            Ok(ForkResult::Parent { .. }) => {}
            Err(e) => eprintln!("Fork failed: {}", e),
        }
    }

    // Close pipes in the parent process after all child processes are done
    for (r, w) in fds {
        close(r.as_raw_fd()).ok();
        close(w.as_raw_fd()).ok();
    }

    if !background {
        for _ in 0..commands.len() {
            match waitpid(Pid::from_raw(-1), None) {
                Ok(_) => {}
                Err(nix::errno::Errno::ECHILD) => break,
                Err(e) => eprintln!("waitpid error: {}", e),
            }
        }
    }
}
