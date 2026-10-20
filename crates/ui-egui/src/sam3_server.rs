//! Existing ComfyUI Desktop installation, started headlessly only when Object Selection needs it.
//! Never installs, repairs, or modifies model files. The selected port is explicit IPv4 loopback.
use std::ffi::OsString;
use std::net::{SocketAddrV4, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The port of a plain `http://127.0.0.1:<port>` address, which is the only kind this app may start a
/// ComfyUI on. Any other address (another host name, https, a path) is somebody else's server.
fn loopback_port(url: &str) -> Option<u16> {
    let port = url.strip_prefix("http://127.0.0.1:")?.trim_end_matches('/');
    match port.parse::<u16>() {
        Ok(0) | Err(_) => None,
        Ok(port) => Some(port),
    }
}

/// Windows process flags for the headless server: no window of its own, in its own process group.
///
/// It must NOT be `DETACHED_PROCESS`. That flag gives the server no console at all, and Windows then
/// ignores `CREATE_NO_WINDOW` and opens a visible console for every console program the server
/// starts in turn (ComfyUI-SAM3 runs its model in one). Without it the server gets a hidden console
/// that its children share, so nothing appears.
#[cfg(target_os = "windows")]
const HEADLESS_FLAGS: u32 = CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP;
#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
#[cfg(target_os = "windows")]
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
#[cfg(all(test, target_os = "windows"))]
const DETACHED_PROCESS: u32 = 0x0000_0008;

/// Makes sure the ComfyUI at `url` (the address the shared Select subject binding chose) answers,
/// starting the installed Desktop one headlessly only when `url` is a plain IPv4 loopback address.
pub(super) fn ensure_running(url: &str, model: crate::sam3_route::SelectionModel) -> Result<(), String> {
    let session = craft_ai_comfyui::Sam3Session::new(url).map_err(|e| e.message)?;
    let Some(port) = loopback_port(url) else {
        return session.verify_server().map_err(|e| format!("The ComfyUI chosen for Select subject is not answering: {}", e.message));
    };
    let address = SocketAddrV4::new(std::net::Ipv4Addr::LOCALHOST, port);
    if TcpStream::connect_timeout(&address.into(), Duration::from_secs(2)).is_ok() {
        return session.verify_server().map_err(|e| format!("ComfyUI port is in use: {}", e.message));
    }
    #[cfg(not(target_os = "windows"))]
    return Err("Start your installed ComfyUI headless on 127.0.0.1:8000 to use Object Selection".into());

    #[cfg(target_os = "windows")]
    {
        let (python, program, base, extra) = desktop_paths(model)?;
        let args = headless_args(&base, &program, &extra, port);
        use std::os::windows::process::CommandExt;
        let mut child = std::process::Command::new(&python)
            .args(args)
            .current_dir(&program)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(HEADLESS_FLAGS)
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

fn installed_model_file(model: crate::sam3_route::SelectionModel) -> &'static str {
    match model {
        crate::sam3_route::SelectionModel::Sam3 => "models/sam3/sam3.safetensors",
        crate::sam3_route::SelectionModel::Sam21 => "models/sam2/sam2.1_hiera_large-fp16.safetensors",
    }
}

#[cfg(target_os = "windows")]
fn desktop_paths(model: crate::sam3_route::SelectionModel) -> Result<(PathBuf, PathBuf, PathBuf, PathBuf), String> {
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
    let model_file = installed_model_file(model);
    if !base.join(model_file).is_file() {
        return Err("The chosen selection model is missing. Set it up in the Craft launcher".into());
    }
    if !base.is_absolute()
        || !base.join("models").is_dir()
        || !base.join("custom_nodes").is_dir()
        || !extra.is_file()
        || !python.is_file()
        || !program.join("main.py").is_file()
        || !program.join("web_custom_versions/desktop_app").is_dir()
    {
        return Err("Your existing ComfyUI Desktop or selection-model installation is incomplete. Set it up in the Craft launcher".into());
    }
    Ok((python, program, base, extra))
}

fn headless_args(base: &Path, program: &Path, extra: &Path, port: u16) -> Vec<OsString> {
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
        port.to_string().into(),
        "--enable-manager".into(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sam21_startup_checks_the_exact_default_precision_backing_file() {
        assert_eq!(installed_model_file(crate::sam3_route::SelectionModel::Sam21), "models/sam2/sam2.1_hiera_large-fp16.safetensors");
    }

    #[test]
    fn headless_launch_uses_the_existing_desktop_resources_and_ipv4_loopback() {
        let base = Path::new("C:/my ComfyUI");
        let program = Path::new("C:/Programs/ComfyUI/resources/ComfyUI");
        let extra = Path::new("C:/AppData/ComfyUI/extra_models_config.yaml");
        let args = headless_args(base, program, extra, 8000);
        assert_eq!(args.first().unwrap(), &program.join("main.py").into_os_string());
        assert!(args.windows(2).any(|pair| pair == ["--listen", "127.0.0.1"]));
        assert!(args.windows(2).any(|pair| pair == ["--port", "8000"]));
        assert!(args.windows(2).any(|pair| pair[0] == "--extra-model-paths-config" && pair[1] == extra.as_os_str()));
        assert!(args.contains(&"--disable-auto-launch".into()));
        assert_eq!(args.iter().filter(|arg| arg.as_os_str() == "--database-url").count(), 1);
    }

    #[test]
    fn the_headless_server_listens_on_the_port_that_was_chosen() {
        let args = headless_args(Path::new("C:/b"), Path::new("C:/p"), Path::new("C:/e.yaml"), 9100);
        assert!(args.windows(2).any(|pair| pair == ["--port", "9100"]));
        assert!(!args.windows(2).any(|pair| pair == ["--port", "8000"]));
    }

    #[test]
    fn only_a_plain_ipv4_loopback_address_may_be_started_here() {
        assert_eq!(loopback_port("http://127.0.0.1:9100"), Some(9100));
        assert_eq!(loopback_port("http://127.0.0.1:9100/"), Some(9100));
        assert_eq!(loopback_port("http://127.0.0.1:8000"), Some(8000));
        for other in [
            "http://localhost:9100",
            "http://[::1]:9100",
            "https://127.0.0.1:9100",
            "http://192.168.0.5:9100",
            "http://127.0.0.1:9100/x",
            "http://127.0.0.1",
            "http://127.0.0.1:0",
            "http://user@127.0.0.1:9100",
        ] {
            assert_eq!(loopback_port(other), None, "{other}");
        }
    }
}

#[cfg(all(test, target_os = "windows"))]
mod windows_flags {
    use super::*;

    #[test]
    fn the_server_is_hidden_without_being_detached_so_its_children_stay_hidden_too() {
        assert_ne!(HEADLESS_FLAGS & CREATE_NO_WINDOW, 0);
        assert_eq!(HEADLESS_FLAGS & DETACHED_PROCESS, 0, "detaching makes Windows open a console for each child");
    }
}
