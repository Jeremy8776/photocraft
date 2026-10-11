//! Existing ComfyUI Desktop installation, started headlessly only when Object Selection needs it.
//! Never installs, repairs, or modifies model files. The selected port is explicit IPv4 loopback.
use std::ffi::OsString;
use std::net::{SocketAddrV4, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const URL: &str = "http://127.0.0.1:8000";
const PORT: u16 = 8000;

pub(super) fn ensure_running() -> Result<(), String> {
    let session = craft_ai_comfyui::Sam3Session::new(URL).map_err(|e| e.message)?;
    let address = SocketAddrV4::new(std::net::Ipv4Addr::LOCALHOST, PORT);
    if TcpStream::connect_timeout(&address.into(), Duration::from_secs(2)).is_ok() {
        return session.verify_server().map_err(|e| format!("ComfyUI port is in use: {}", e.message));
    }
    #[cfg(not(target_os = "windows"))]
    return Err("Start your installed ComfyUI headless on 127.0.0.1:8000 to use Object Selection".into());

    #[cfg(target_os = "windows")]
    {
        let (python, program, base, extra) = desktop_paths()?;
        let args = headless_args(&base, &program, &extra);
        use std::os::windows::process::CommandExt;
        let mut child = std::process::Command::new(&python)
            .args(args)
            .current_dir(&program)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(0x0800_0000 | 0x0000_0008 | 0x0000_0200)
            .spawn()
            .map_err(|e| format!("Could not start your existing headless ComfyUI: {e}"))?;
        let deadline = Instant::now() + Duration::from_secs(60);
        while Instant::now() < deadline {
            if session.verify_server().is_ok() {
                return Ok(());
            }
            if child.try_wait().map_err(|e| format!("Could not check ComfyUI startup: {e}"))?.is_some() {
                return Err("Your existing ComfyUI closed during headless startup. Open ComfyUI Desktop to inspect its logs".into());
            }
            std::thread::sleep(Duration::from_millis(350));
        }
        Err("Your existing ComfyUI did not become ready within 60 seconds".into())
    }
}

#[cfg(target_os = "windows")]
fn desktop_paths() -> Result<(PathBuf, PathBuf, PathBuf, PathBuf), String> {
    let roaming = PathBuf::from(std::env::var_os("APPDATA").ok_or("Windows APPDATA is unavailable")?).join("ComfyUI");
    let local = PathBuf::from(std::env::var_os("LOCALAPPDATA").ok_or("Windows LOCALAPPDATA is unavailable")?);
    let config = roaming.join("config.json");
    let metadata = std::fs::metadata(&config).map_err(|_| "The existing ComfyUI Desktop settings were not found")?;
    if !metadata.is_file() || metadata.len() > 256 * 1024 {
        return Err("The existing ComfyUI Desktop settings are invalid".into());
    }
    let data: serde_json::Value =
        serde_json::from_slice(&std::fs::read(config).map_err(|e| e.to_string())?).map_err(|_| "The existing ComfyUI Desktop settings are invalid")?;
    let base = PathBuf::from(data.get("basePath").and_then(serde_json::Value::as_str).ok_or("The existing ComfyUI Desktop base path is missing")?);
    let program = local.join("Programs/ComfyUI/resources/ComfyUI");
    let extra = roaming.join("extra_models_config.yaml");
    let python = base.join(".venv/Scripts/python.exe");
    if !base.is_absolute()
        || !base.join("models/sam3").is_dir()
        || !base.join("custom_nodes/comfyui-sam3").is_dir()
        || !extra.is_file()
        || !python.is_file()
        || !program.join("main.py").is_file()
        || !program.join("web_custom_versions/desktop_app").is_dir()
    {
        return Err("Your existing ComfyUI Desktop or SAM 3 installation is incomplete. Set it up in the Craft launcher".into());
    }
    Ok((python, program, base, extra))
}

fn headless_args(base: &Path, program: &Path, extra: &Path) -> Vec<OsString> {
    let database = format!("sqlite:///{}", base.join("user/comfyui.db").to_string_lossy().replace('\\', "/"));
    vec![
        program.join("main.py").into_os_string(),
        "--user-directory".into(),
        base.join("user").into_os_string(),
        "--input-directory".into(),
        base.join("input").into_os_string(),
        "--output-directory".into(),
        base.join("output").into_os_string(),
        "--front-end-root".into(),
        program.join("web_custom_versions/desktop_app").into_os_string(),
        "--base-directory".into(),
        base.as_os_str().to_owned(),
        "--database-url".into(),
        database.into(),
        "--extra-model-paths-config".into(),
        extra.as_os_str().to_owned(),
        "--disable-auto-launch".into(),
        "--listen".into(),
        "127.0.0.1".into(),
        "--port".into(),
        PORT.to_string().into(),
        "--enable-manager".into(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headless_launch_uses_the_existing_desktop_resources_and_ipv4_loopback() {
        let base = Path::new("C:/my ComfyUI");
        let program = Path::new("C:/Programs/ComfyUI/resources/ComfyUI");
        let extra = Path::new("C:/AppData/ComfyUI/extra_models_config.yaml");
        let args = headless_args(base, program, extra);
        assert_eq!(args.first().unwrap(), &program.join("main.py").into_os_string());
        assert!(args.windows(2).any(|pair| pair == ["--listen", "127.0.0.1"]));
        assert!(args.windows(2).any(|pair| pair == ["--port", "8000"]));
        assert!(args.windows(2).any(|pair| pair[0] == "--extra-model-paths-config" && pair[1] == extra.as_os_str()));
        assert!(args.contains(&"--disable-auto-launch".into()));
        assert_eq!(args.iter().filter(|arg| arg.as_os_str() == "--database-url").count(), 1);
    }
}
