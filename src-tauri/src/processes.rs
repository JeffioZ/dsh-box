//! 跨平台进程管理：
//! - Windows：Job 对象（KILL_ON_JOB_CLOSE）、CREATE_NO_WINDOW 隐藏窗口、taskkill 清理；
//! - macOS/Linux：进程组（setpgid）+ kill 进程组回收、无控制台概念、open/xdg-open。

use std::io;
use std::path::Path;
#[cfg(windows)]
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

#[cfg(unix)]
use std::os::unix::process::CommandExt;
#[cfg(windows)]
use std::os::windows::process::CommandExt;

/// CREATE_NO_WINDOW：子进程不创建控制台窗口（Windows，杜绝终端闪现）。
#[cfg(windows)]
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 隐藏命令的控制台窗口（Windows）；其他平台无此概念，为无操作。
pub fn hide_console(cmd: &mut Command) {
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    #[cfg(not(windows))]
    let _ = cmd;
}

/// 构造 pwsh 启动命令（Windows）：优先绝对安装路径——应用进程的 PATH
/// 是启动时的快照，之后才安装的 pwsh 不在其中，按名查找会漏检；
/// 找不到再退回 PATH 查找。
#[cfg(windows)]
pub fn pwsh_command() -> Command {
    let exe = find_pwsh().unwrap_or_else(|| PathBuf::from("pwsh"));
    Command::new(exe)
}

/// 定位 pwsh.exe：机器级/用户级标准安装路径优先，PATH 兜底。
#[cfg(windows)]
pub fn find_pwsh() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(p) = std::env::var("ProgramFiles") {
        candidates.push(
            PathBuf::from(p)
                .join("PowerShell")
                .join("7")
                .join("pwsh.exe"),
        );
    }
    if let Ok(p) = std::env::var("LOCALAPPDATA") {
        candidates.push(
            PathBuf::from(p)
                .join("Microsoft")
                .join("PowerShell")
                .join("7")
                .join("pwsh.exe"),
        );
    }
    if let Some(exe) = candidates.into_iter().find(|p| p.exists()) {
        return Some(exe);
    }
    // PATH 兜底（应用启动时已在 PATH 中的场景）
    let mut cmd = Command::new("where");
    cmd.arg("pwsh");
    hide_console(&mut cmd);
    if let Ok(out) = cmd.output() {
        let stdout = String::from_utf8_lossy(&out.stdout);
        let first = stdout.lines().map(str::trim).find(|line| !line.is_empty());
        if let Some(line) = first {
            let path = PathBuf::from(line);
            if path.exists() {
                return Some(path);
            }
        }
    }
    None
}

/// 进程树守卫：
/// - Windows：Job 对象，句柄关闭（应用退出）时强制回收整棵树；
/// - Unix：进程组（子进程以独立进程组启动），守卫销毁时 TERM 整个进程组。
pub struct TreeGuard {
    #[cfg(windows)]
    job: Option<win::Job>,
    #[cfg(unix)]
    pgid: i32,
}

impl TreeGuard {
    /// 为子进程建立守卫。子进程必须经本模块 spawn（Windows 由 Job 加入；
    /// Unix 由 process_group(0) 使 pgid == pid）。
    /// Windows 下 Job 创建/加入失败时返回 None：调用方据此走按 PID 的
    /// kill_tree 兜底，而不是拿到一个回收不了任何进程的空守卫。
    pub fn from_child(child: &Child) -> Option<TreeGuard> {
        #[cfg(windows)]
        {
            win::job_for_process(child.id()).map(|job| TreeGuard { job: Some(job) })
        }
        #[cfg(unix)]
        {
            Some(TreeGuard {
                pgid: child.id() as i32,
            })
        }
    }
}

impl Drop for TreeGuard {
    fn drop(&mut self) {
        #[cfg(windows)]
        {
            // job 字段 Drop 时关闭句柄 → KILL_ON_JOB_CLOSE 回收整棵树
            let _ = self.job.take();
        }
        #[cfg(unix)]
        {
            // 终止整个进程组（含 dsh 子进程）；已退出的组为无害错误
            let _ = Command::new("kill")
                .arg("-TERM")
                .arg(format!("-{}", self.pgid))
                .spawn();
        }
    }
}

#[cfg(windows)]
mod win {
    use super::*;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
    };

    /// 一个 Windows Job 对象：句柄关闭时强制杀死其下全部进程（含后代）。
    pub struct Job {
        handle: HANDLE,
    }

    // HANDLE 是 isize，可安全跨线程传递；Tauri 后端线程会持有它。
    unsafe impl Send for Job {}

    impl Job {
        /// 创建启用了 KILL_ON_JOB_CLOSE 的 Job。
        pub fn new() -> io::Result<Job> {
            unsafe {
                let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if handle.is_null() {
                    return Err(io::Error::last_os_error());
                }
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let ok = SetInformationJobObject(
                    handle,
                    JobObjectExtendedLimitInformation,
                    &info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION as *const _,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                if ok == 0 {
                    CloseHandle(handle);
                    return Err(io::Error::last_os_error());
                }
                Ok(Job { handle })
            }
        }

        /// 把指定进程加入 Job。
        pub fn assign(&self, pid: u32) -> io::Result<()> {
            unsafe {
                let process = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
                if process.is_null() {
                    return Err(io::Error::last_os_error());
                }
                let ok = AssignProcessToJobObject(self.handle, process);
                CloseHandle(process);
                if ok == 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.handle);
            }
        }
    }

    /// 为进程创建独立 Job；失败时允许命令继续，但记录降级原因。
    pub fn job_for_process(pid: u32) -> Option<Job> {
        match Job::new() {
            Ok(job) => match job.assign(pid) {
                Ok(()) => Some(job),
                Err(e) => {
                    crate::logging::log(&format!(
                        "进程: 进程 {pid} 加入 Job 失败，将降级运行：{e}"
                    ));
                    None
                }
            },
            Err(e) => {
                crate::logging::log(&format!(
                    "进程: 为进程 {pid} 创建 Job 失败，将降级运行：{e}"
                ));
                None
            }
        }
    }
}

/// 隐藏窗口启动子进程；stdout/stderr 追加写入日志文件（可选）。
/// Unix 下子进程放入独立进程组（便于整体回收）。
pub fn spawn_process(
    program: &Path,
    args: &[String],
    envs: &[(&str, String)],
    cwd: Option<&Path>,
    log: Option<&Path>,
) -> io::Result<Child> {
    let mut cmd = Command::new(program);
    cmd.args(args);
    hide_console(&mut cmd);
    #[cfg(unix)]
    cmd.process_group(0);
    if let Some(c) = cwd {
        cmd.current_dir(c);
    }
    if let Some(log_path) = log {
        if let Some(dir) = log_path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path)?;
        let file2 = file.try_clone()?;
        cmd.stdout(Stdio::from(file)).stderr(Stdio::from(file2));
    } else {
        cmd.stdout(Stdio::null()).stderr(Stdio::null());
    }
    cmd.envs(envs.iter().map(|(k, v)| (*k, v.clone())));
    cmd.spawn()
}

/// 捕获子进程 stdout/stderr 并等待结束（用于 winget 等短命令）。
#[cfg(windows)]
pub fn run_capture(
    program: &Path,
    args: &[String],
    envs: &[(&str, String)],
    cwd: Option<&Path>,
) -> io::Result<(i32, String, String)> {
    let mut cmd = Command::new(program);
    cmd.args(args);
    hide_console(&mut cmd);
    if let Some(c) = cwd {
        cmd.current_dir(c);
    }
    cmd.envs(envs.iter().map(|(k, v)| (*k, v.clone())));
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let child = cmd.spawn()?;
    // 短命令也放入守卫；应用异常退出时回收其进程树（Windows Job / Unix 进程组）。
    let _guard = TreeGuard::from_child(&child);
    let output = child.wait_with_output()?;
    Ok((
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    ))
}

/// 强杀进程树：Windows 用 taskkill /T /F；Unix 用 TERM 整个进程组。
pub fn kill_tree(pid: u32) {
    #[cfg(windows)]
    {
        let args = vec![
            "/PID".to_string(),
            pid.to_string(),
            "/T".to_string(),
            "/F".to_string(),
        ];
        let mut cmd = Command::new("taskkill");
        cmd.args(&args);
        hide_console(&mut cmd);
        let _ = cmd.spawn();
    }
    #[cfg(unix)]
    {
        let mut cmd = Command::new("kill");
        cmd.arg("-TERM").arg(format!("-{pid}"));
        let _ = cmd.spawn();
    }
}

/// kill_tree 的强杀升级：TERM 宽限后仍存活的进程使用（Windows 与 kill_tree
/// 同为 /F；Unix 从 -TERM 升级为 -KILL）。
pub fn kill_tree_force(pid: u32) {
    #[cfg(windows)]
    {
        kill_tree(pid);
    }
    #[cfg(unix)]
    {
        let mut cmd = Command::new("kill");
        cmd.arg("-KILL").arg(format!("-{pid}"));
        let _ = cmd.spawn();
    }
}

/// 打开目录：Windows 资源管理器；macOS Finder；Linux 默认文件管理器。
pub fn open_in_file_manager(dir: &Path) {
    #[cfg(windows)]
    {
        let mut cmd = Command::new("explorer.exe");
        cmd.arg(dir.as_os_str());
        hide_console(&mut cmd);
        let _ = cmd.spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let mut cmd = Command::new("open");
        cmd.arg(dir.as_os_str());
        let _ = cmd.spawn();
    }
    #[cfg(target_os = "linux")]
    {
        let mut cmd = Command::new("xdg-open");
        cmd.arg(dir.as_os_str());
        let _ = cmd.spawn();
    }
}
