mod app;
mod asus;
mod cli;
mod config;
mod deps;
mod gpu;
mod hw;
mod nixos;
mod nm;
mod services;
mod sys;
mod ui;
mod util;

use clap::Parser;
use cli::Cli;

fn main() {
    // 恢复默认 SIGPIPE 行为，避免 `rogtui ... | head` 时 panic。
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }

    let cli = Cli::parse();

    // 只读子命令不需要 root：doctor 只探测环境，nixos 只读 /etc 与导出。
    let readonly = matches!(
        &cli.command,
        Some(
            cli::Cmd::Doctor
                | cli::Cmd::Nixos { .. }
                | cli::Cmd::Gpu {
                    cmd: cli::GpuCmd::Topology
                }
        )
    );
    if !readonly && !util::is_root() {
        eprintln!("rogtui 需要 root 权限运行，请使用：sudo rogtui");
        std::process::exit(1);
    }

    let cfg = match config::Config::load() {
        Ok(cfg) => cfg,
        Err(err) => {
            eprintln!("读取配置失败：{err:#}");
            std::process::exit(1);
        }
    };

    let deps = deps::check();
    let result = match cli.command {
        None => app::run(cfg, deps),
        Some(cmd) => {
            if !matches!(
                cmd,
                cli::Cmd::Doctor
                    | cli::Cmd::Nixos { .. }
                    | cli::Cmd::Gpu {
                        cmd: cli::GpuCmd::Topology
                    }
            ) && let Some(warning) = deps.startup_warning()
            {
                eprintln!("{warning}");
            }
            cli::run(cmd, cli.json)
        }
    };

    if let Err(err) = result {
        eprintln!("错误：{err:#}");
        std::process::exit(1);
    }
}
