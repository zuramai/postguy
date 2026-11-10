//! Installing an app update: work out how this copy of Postguy was installed, name the release
//! asset that updates it, unpack and verify a downloaded package, and swap it in after the app
//! has closed (then start the new version).
//!
//! The engine downloads the asset and checks its SHA-256 against the release's checksum list.
//! On top of that, when the running app is code-signed (macOS Developer ID, Windows
//! Authenticode), the new one must carry a valid signature from the same signer; otherwise
//! nothing is installed. Linux packages and unsigned builds rely on the checksum alone, and
//! [`SignatureCheck::NotChecked`] says so.
//!
//! Package-manager installs (Flatpak, deb, rpm) are never touched: they update through their
//! package manager. A development build (not running from an installed package) cannot update
//! itself either.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

/// How the running app was installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Install {
    /// `Postguy.app` (from the DMG or the zip).
    MacApp { bundle: PathBuf },
    /// The Windows MSI, installed per machine.
    WindowsMsi { exe: PathBuf },
    /// The Windows portable zip, unpacked into `dir`.
    WindowsPortable { dir: PathBuf },
    /// A Linux AppImage file.
    AppImage { file: PathBuf },
    /// The Linux `.tar.gz` tree; `root` holds `bin/postguy`.
    LinuxTarball { root: PathBuf },
    /// Updated by a package manager; `message` says how.
    Managed { message: String },
    /// No in-app install for this copy; `message` says why.
    Unsupported { message: String },
}

/// What the signature check on a prepared update established.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignatureCheck {
    /// The new version is validly signed by the running app's signer.
    Matched { signer: String },
    /// No signature comparison was possible; only the checksum vouches for the download.
    NotChecked { reason: String },
}

impl SignatureCheck {
    pub fn describe(&self) -> String {
        match self {
            SignatureCheck::Matched { signer } => format!("Checksum and signature verified ({signer})"),
            SignatureCheck::NotChecked { reason } => format!("Checksum verified. {reason}"),
        }
    }
}

/// A verified update, ready to replace the installed app once it has closed.
#[derive(Debug)]
pub struct PreparedUpdate {
    pub signature: SignatureCheck,
    step: Step,
}

#[derive(Debug)]
enum Step {
    /// Replace `bundle` with `new_app`, then open it.
    MacBundle { bundle: PathBuf, new_app: PathBuf },
    /// Run the MSI after this process exits, then start `exe`.
    WindowsInstaller { msi: PathBuf, exe: PathBuf },
    /// Replace each `(new, installed)` file, then start `relaunch`.
    ReplaceFiles { files: Vec<(PathBuf, PathBuf)>, relaunch: PathBuf },
}

/// The per-user folder for update files. `/tmp` on Linux is shared with every account, so a
/// download there could be swapped between its checksum and its install; macOS and Windows
/// temp folders are already per user.
pub fn staging_root() -> PathBuf {
    if cfg!(any(target_os = "macos", windows)) {
        return std::env::temp_dir().join("postguy-update");
    }
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")));
    cache.unwrap_or_else(std::env::temp_dir).join("postguy-update")
}

/// Where this process keeps downloads and unpacked packages: one folder per process, so two
/// running copies never share one.
pub fn staging_dir() -> PathBuf {
    staging_root().join(std::process::id().to_string())
}

/// Remove what earlier updates left behind: staging folders older than a day (a Windows
/// installer may still be reading a younger one) and the `*.old` program copies Windows could
/// not delete while they were running. Best effort.
pub fn cleanup_previous() {
    let day = std::time::Duration::from_secs(24 * 60 * 60);
    if let Ok(entries) = std::fs::read_dir(staging_root()) {
        for entry in entries.flatten() {
            let stale =
                entry.metadata().and_then(|meta| meta.modified()).ok().and_then(|modified| modified.elapsed().ok()).is_some_and(|age| age > day);
            if stale {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        for name in ["postguy.exe", "postguy-cli.exe", "postguy", "postguy-cli"] {
            let _ = std::fs::remove_file(dir.join(format!("{name}.old")));
        }
    }
}

impl Install {
    /// Detect how this process was installed.
    pub fn detect() -> Install {
        let exe = match std::env::current_exe().and_then(|exe| exe.canonicalize()) {
            Ok(exe) => exe,
            Err(err) => return Install::Unsupported { message: format!("Postguy could not find its own program file ({err}).") },
        };
        detect_for(&exe)
    }

    /// The release asset that updates this install, for release `version`.
    pub fn asset_name(&self, version: &str) -> Option<String> {
        match self {
            Install::MacApp { .. } => Some(format!("postguy-{version}-macos-universal.zip")),
            Install::WindowsMsi { .. } => windows_arch().map(|arch| format!("postguy-{version}-windows-{arch}.msi")),
            Install::WindowsPortable { .. } => windows_arch().map(|arch| format!("postguy-{version}-windows-{arch}.zip")),
            Install::AppImage { .. } => linux_arch().map(|arch| format!("postguy-{version}-linux-{arch}.AppImage")),
            Install::LinuxTarball { .. } => linux_arch().map(|arch| format!("postguy-{version}-linux-{arch}.tar.gz")),
            Install::Managed { .. } | Install::Unsupported { .. } => None,
        }
    }

    /// Why this copy cannot install updates itself (`None` when it can).
    pub fn manual_reason(&self) -> Option<String> {
        match self {
            Install::Managed { message } | Install::Unsupported { message } => Some(message.clone()),
            other if other.asset_name("0.0.0").is_none() => Some("There is no release package for this processor.".into()),
            _ => None,
        }
    }

    /// Unpack and verify the downloaded `package`. Nothing installed changes here.
    pub fn prepare(&self, package: &Path) -> Result<PreparedUpdate, String> {
        let unpacked = staging_dir().join("unpacked");
        let _ = std::fs::remove_dir_all(&unpacked);
        std::fs::create_dir_all(&unpacked).map_err(|err| format!("creating {}: {err}", unpacked.display()))?;
        match self {
            Install::MacApp { bundle } => prepare_mac(bundle, package, &unpacked),
            Install::WindowsMsi { exe } => {
                let signature = compare_signers(&authenticode_signer(exe), &authenticode_signer(package), "Authenticode")?;
                Ok(PreparedUpdate { signature, step: Step::WindowsInstaller { msi: package.to_path_buf(), exe: exe.clone() } })
            }
            Install::WindowsPortable { dir } => prepare_windows_zip(dir, package, &unpacked),
            Install::AppImage { file } => {
                let dir = file.parent().ok_or_else(|| format!("{} has no folder", file.display()))?;
                ensure_writable(dir)?;
                make_executable(package)?;
                Ok(PreparedUpdate {
                    signature: unsigned_linux(),
                    step: Step::ReplaceFiles { files: vec![(package.to_path_buf(), file.clone())], relaunch: file.clone() },
                })
            }
            Install::LinuxTarball { root } => prepare_tarball(root, package, &unpacked),
            Install::Managed { message } | Install::Unsupported { message } => Err(message.clone()),
        }
    }
}

impl PreparedUpdate {
    /// Swap the new version in and start it with `args` (the arguments this process got). Call
    /// it after the window closed and the session was saved; the caller exits right after.
    pub fn apply_and_relaunch(self, args: Vec<OsString>) -> Result<(), String> {
        match self.step {
            Step::MacBundle { bundle, new_app } => {
                replace_bundle(&bundle, &new_app)?;
                let mut open = Command::new("open");
                open.arg("-n").arg(&bundle);
                if !args.is_empty() {
                    open.arg("--args").args(&args);
                }
                spawn(&mut open, "starting the updated app")
            }
            Step::WindowsInstaller { msi, exe } => run_msi_then_relaunch(&msi, &exe, &args),
            Step::ReplaceFiles { files, relaunch } => {
                let mut done: Vec<(PathBuf, PathBuf)> = Vec::new();
                for (new, installed) in &files {
                    match replace_file(new, installed) {
                        Ok(old) => done.push((old, installed.clone())),
                        Err(err) => {
                            // Put back what was already swapped, so the install stays one version.
                            for (old, installed) in done.iter().rev() {
                                let _ = std::fs::rename(old, installed);
                            }
                            return Err(err);
                        }
                    }
                }
                for (old, _) in &done {
                    // Windows keeps a running program locked; `cleanup_previous` removes it later.
                    let _ = std::fs::remove_file(old);
                }
                spawn(Command::new(&relaunch).args(&args), "starting the updated app")
            }
        }
    }
}

fn detect_for(exe: &Path) -> Install {
    let dev = || {
        Install::Unsupported {
        message: "This copy of Postguy is not an installed release (a development build), so it cannot update itself. Download the new version from the release page."
            .into(),
    }
    };
    if cfg!(target_os = "macos") {
        let bundle = exe.parent().filter(|dir| dir.ends_with("Contents/MacOS")).and_then(Path::parent).and_then(Path::parent);
        return match bundle {
            Some(bundle) if bundle.extension().is_some_and(|ext| ext == "app") => Install::MacApp { bundle: bundle.to_path_buf() },
            _ => dev(),
        };
    }
    if cfg!(windows) {
        let Some(dir) = exe.parent() else { return dev() };
        // Both the MSI and the portable zip put the licences beside the program.
        if !dir.join("LICENSE-MIT").is_file() {
            return dev();
        }
        return if is_msi_install(dir) {
            Install::WindowsMsi { exe: exe.to_path_buf() }
        } else {
            Install::WindowsPortable { dir: dir.to_path_buf() }
        };
    }
    if cfg!(target_os = "linux") {
        if std::env::var_os("FLATPAK_ID").is_some() || Path::new("/.flatpak-info").exists() {
            return Install::Managed {
                message: "This copy was installed with Flatpak. Update it with `flatpak update io.github.zuramai.postguy` or your software centre."
                    .into(),
            };
        }
        if let Some(file) = std::env::var_os("APPIMAGE").map(PathBuf::from).filter(|file| file.is_file()) {
            return Install::AppImage { file };
        }
        if exe.starts_with("/usr") {
            return Install::Managed {
                message: "This copy was installed by your package manager (deb or rpm). Update it with apt, dnf, zypper or your software centre."
                    .into(),
            };
        }
        let root = exe.parent().filter(|dir| dir.ends_with("bin")).and_then(Path::parent);
        return match root {
            Some(root) if root.join("share/applications/io.github.zuramai.postguy.desktop").is_file() => {
                Install::LinuxTarball { root: root.to_path_buf() }
            }
            _ => dev(),
        };
    }
    Install::Unsupported { message: "There are no Postguy release packages for this operating system; update it the way you installed it.".into() }
}

fn windows_arch() -> Option<&'static str> {
    if !cfg!(windows) {
        return None;
    }
    match std::env::consts::ARCH {
        "x86_64" => Some("x64"),
        "x86" => Some("x86"),
        "aarch64" => Some("arm64"),
        _ => None,
    }
}

fn linux_arch() -> Option<&'static str> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    match std::env::consts::ARCH {
        "x86_64" => Some("x86_64"),
        "aarch64" => Some("aarch64"),
        _ => None,
    }
}

fn unsigned_linux() -> SignatureCheck {
    SignatureCheck::NotChecked { reason: "Linux packages are not code-signed.".into() }
}

/// Run a tool to completion; a failure carries its stderr.
fn run(command: &mut Command, what: &str) -> Result<std::process::Output, String> {
    hide_console(command);
    let output = command.output().map_err(|err| format!("{what}: {err}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("{what} failed: {}", stderr.trim()));
    }
    Ok(output)
}

fn spawn(command: &mut Command, what: &str) -> Result<(), String> {
    hide_console(command);
    command.spawn().map(|_| ()).map_err(|err| format!("{what}: {err}"))
}

#[cfg(windows)]
fn hide_console(command: &mut Command) {
    use std::os::windows::process::CommandExt as _;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_console(_command: &mut Command) {}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).map_err(|err| format!("making {} executable: {err}", path.display()))
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<(), String> {
    Ok(())
}

/// Refuse early, with advice, when the install folder cannot be written.
fn ensure_writable(dir: &Path) -> Result<(), String> {
    let probe = dir.join(format!(".postguy-update-probe-{}", std::process::id()));
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            Ok(())
        }
        Err(err) => Err(format!(
            "Postguy cannot write to {} ({err}). Move it to a folder you can write to, or install the update from the release page.",
            dir.display()
        )),
    }
}

/// Copy `new` beside `installed`, move `installed` to `<name>.old` and the copy into its place.
/// Returns the `.old` path; on failure the installed file is back where it was.
fn replace_file(new: &Path, installed: &Path) -> Result<PathBuf, String> {
    let name = installed.file_name().ok_or_else(|| format!("{} has no file name", installed.display()))?.to_string_lossy().into_owned();
    let staged = installed.with_file_name(format!("{name}.new"));
    let old = installed.with_file_name(format!("{name}.old"));
    std::fs::copy(new, &staged).map_err(|err| format!("copying the update to {}: {err}", staged.display()))?;
    make_executable(&staged)?;
    let _ = std::fs::remove_file(&old);
    if let Err(err) = std::fs::rename(installed, &old) {
        let _ = std::fs::remove_file(&staged);
        return Err(format!("moving {} aside: {err}", installed.display()));
    }
    if let Err(err) = std::fs::rename(&staged, installed) {
        let _ = std::fs::rename(&old, installed);
        let _ = std::fs::remove_file(&staged);
        return Err(format!("putting the update at {}: {err}", installed.display()));
    }
    Ok(old)
}

/// Compare the running app's signer with the new package's. An unsigned running app checks
/// nothing; a signed one requires the same valid signer.
fn compare_signers(running: &Option<String>, new: &Option<String>, kind: &str) -> Result<SignatureCheck, String> {
    match (running, new) {
        (None, _) => Ok(SignatureCheck::NotChecked { reason: format!("This copy of Postguy has no {kind} signature to compare with.") }),
        (Some(running), Some(new)) if running == new => Ok(SignatureCheck::Matched { signer: new.clone() }),
        (Some(running), Some(new)) => Err(format!("The download is signed by `{new}`, not `{running}`. Nothing was installed.")),
        (Some(running), None) => Err(format!("The download has no valid {kind} signature from `{running}`. Nothing was installed.")),
    }
}

// ---- macOS ----------------------------------------------------------------------------------

fn prepare_mac(bundle: &Path, package: &Path, unpacked: &Path) -> Result<PreparedUpdate, String> {
    let parent = bundle.parent().ok_or_else(|| format!("{} has no folder", bundle.display()))?;
    ensure_writable(parent)?;
    run(Command::new("ditto").arg("-x").arg("-k").arg(package).arg(unpacked), "unpacking the update")?;
    let new_app = unpacked.join("Postguy.app");
    if !new_app.join("Contents/MacOS/Postguy").is_file() {
        return Err("The downloaded archive does not contain Postguy.app. Nothing was installed.".into());
    }
    let running = mac_team(bundle);
    if running.is_some() {
        run(Command::new("codesign").args(["--verify", "--strict", "--deep"]).arg(&new_app), "checking the update's signature")
            .map_err(|err| format!("{err}. Nothing was installed."))?;
    }
    let signature = compare_signers(&running, &mac_team(&new_app).filter(|_| running.is_some()), "Developer ID")?;
    Ok(PreparedUpdate { signature, step: Step::MacBundle { bundle: bundle.to_path_buf(), new_app } })
}

/// The Developer ID team of a signed bundle (`None` when unsigned or ad-hoc signed).
fn mac_team(path: &Path) -> Option<String> {
    let mut command = Command::new("codesign");
    command.args(["-dv", "--verbose=2"]).arg(path);
    let output = command.output().ok()?;
    // codesign prints the details on stderr.
    let text = String::from_utf8_lossy(&output.stderr);
    let team = text.lines().find_map(|line| line.strip_prefix("TeamIdentifier="))?.trim();
    (!team.is_empty() && team != "not set").then(|| format!("Apple team {team}"))
}

/// Put `new_app` where `bundle` is. The copy goes next to the bundle first (same volume), so
/// the swap is two renames; if the second fails the old bundle is renamed back.
fn replace_bundle(bundle: &Path, new_app: &Path) -> Result<(), String> {
    let parent = bundle.parent().ok_or_else(|| format!("{} has no folder", bundle.display()))?;
    let staged = parent.join(".Postguy-update.app");
    let old = parent.join(".Postguy-old.app");
    let _ = std::fs::remove_dir_all(&staged);
    let _ = std::fs::remove_dir_all(&old);
    run(Command::new("ditto").arg(new_app).arg(&staged), "copying the update into place")?;
    if let Err(err) = std::fs::rename(bundle, &old) {
        let _ = std::fs::remove_dir_all(&staged);
        return Err(format!("moving {} aside: {err}", bundle.display()));
    }
    if let Err(err) = std::fs::rename(&staged, bundle) {
        let _ = std::fs::rename(&old, bundle);
        let _ = std::fs::remove_dir_all(&staged);
        return Err(format!("putting the update at {}: {err}", bundle.display()));
    }
    let _ = std::fs::remove_dir_all(&old);
    Ok(())
}

// ---- Windows --------------------------------------------------------------------------------

/// The MSI records its folder under `HKLM\Software\Postguy` (`InstallDir`); installs from before
/// that value existed are recognised by the default Program Files folder.
fn is_msi_install(dir: &Path) -> bool {
    // `canonicalize` gives `\\?\C:\...` on Windows; the registry and environment do not.
    let same = |other: &Path| {
        let normalize = |path: &Path| path.to_string_lossy().trim_start_matches(r"\\?\").trim_end_matches(['\\', '/']).to_ascii_lowercase();
        normalize(dir) == normalize(other)
    };
    let view = if cfg!(target_arch = "x86") { "/reg:32" } else { "/reg:64" };
    let mut query = Command::new("reg");
    query.args(["query", r"HKLM\Software\Postguy", "/v", "InstallDir", view]);
    if let Ok(output) = run(&mut query, "reading the install folder") {
        let text = String::from_utf8_lossy(&output.stdout);
        if let Some(value) = text.lines().find_map(|line| line.trim().strip_prefix("InstallDir")).and_then(|rest| rest.trim().strip_prefix("REG_SZ"))
        {
            return same(Path::new(value.trim()));
        }
    }
    ["ProgramW6432", "ProgramFiles", "ProgramFiles(x86)"]
        .iter()
        .filter_map(std::env::var_os)
        .any(|program_files| same(&PathBuf::from(program_files).join("Postguy")))
}

/// The certificate subject of a valid Authenticode signature (`None` when unsigned or invalid).
fn authenticode_signer(path: &Path) -> Option<String> {
    if !cfg!(windows) {
        return None;
    }
    let mut command = Command::new("powershell.exe");
    command
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "$s = Get-AuthenticodeSignature -LiteralPath $env:POSTGUY_SIGNED_PATH; \"$($s.Status)|$($s.SignerCertificate.Subject)\"",
        ])
        .env("POSTGUY_SIGNED_PATH", path);
    let output = run(&mut command, "reading the Authenticode signature").ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let (status, subject) = text.trim().split_once('|')?;
    (status == "Valid" && !subject.is_empty()).then(|| subject.to_string())
}

fn prepare_windows_zip(dir: &Path, package: &Path, unpacked: &Path) -> Result<PreparedUpdate, String> {
    ensure_writable(dir)?;
    let file = std::fs::File::open(package).map_err(|err| format!("opening {}: {err}", package.display()))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|err| format!("reading the update archive: {err}"))?;
    let mut files = Vec::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|err| format!("reading the update archive: {err}"))?;
        let Some(name) = entry.enclosed_name().and_then(|path| path.file_name().map(|name| name.to_string_lossy().into_owned())) else { continue };
        // The app always; the CLI only where it is installed beside it. The first copy wins.
        let wanted = name == "postguy.exe" || (name == "postguy-cli.exe" && dir.join(&name).is_file());
        if !entry.is_file() || !wanted || files.iter().any(|(_, installed): &(PathBuf, PathBuf)| installed.ends_with(&name)) {
            continue;
        }
        let target = unpacked.join(&name);
        let mut out = std::fs::File::create(&target).map_err(|err| format!("creating {}: {err}", target.display()))?;
        let cap = UNPACKED_PROGRAM_CAP;
        let copied =
            std::io::copy(&mut std::io::Read::take(&mut entry, cap.saturating_add(1)), &mut out).map_err(|err| format!("unpacking {name}: {err}"))?;
        if copied > cap {
            return Err(format!("{name} in the update archive is larger than {cap} bytes. Nothing was installed."));
        }
        files.push((target, dir.join(&name)));
    }
    let app = files.iter().find(|(_, installed)| installed.ends_with("postguy.exe")).map(|(new, _)| new.clone());
    let Some(app) = app else { return Err("The downloaded archive does not contain postguy.exe. Nothing was installed.".into()) };
    let exe = dir.join("postguy.exe");
    let signature = compare_signers(&authenticode_signer(&exe), &authenticode_signer(&app), "Authenticode")?;
    Ok(PreparedUpdate { signature, step: Step::ReplaceFiles { files, relaunch: exe } })
}

/// The largest single program file an update archive may unpack.
const UNPACKED_PROGRAM_CAP: u64 = 256 << 20;

/// Quote one argument for a Windows command line (the rules `CommandLineToArgvW` undoes).
fn quote_windows_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '\n', '\u{b}', '"']) {
        return arg.to_string();
    }
    let mut quoted = String::from('"');
    let mut backslashes: usize = 0;
    for ch in arg.chars() {
        match ch {
            '\\' => backslashes = backslashes.saturating_add(1),
            '"' => {
                quoted.extend(std::iter::repeat_n('\\', backslashes.saturating_mul(2).saturating_add(1)));
                quoted.push('"');
                backslashes = 0;
            }
            other => {
                quoted.extend(std::iter::repeat_n('\\', backslashes));
                quoted.push(other);
                backslashes = 0;
            }
        }
    }
    quoted.extend(std::iter::repeat_n('\\', backslashes.saturating_mul(2)));
    quoted.push('"');
    quoted
}

/// Hand the MSI to a detached PowerShell that waits for this process to exit, runs the
/// installer (Windows asks for elevation) and starts whatever is installed afterwards, so a
/// cancelled installer still brings the old version back.
fn run_msi_then_relaunch(msi: &Path, exe: &Path, args: &[OsString]) -> Result<(), String> {
    let line = args.iter().map(|arg| quote_windows_arg(&arg.to_string_lossy())).collect::<Vec<_>>().join(" ");
    let script = "Wait-Process -Id $env:POSTGUY_UPDATE_PID -ErrorAction SilentlyContinue; \
        Start-Process -FilePath msiexec.exe -ArgumentList ('/i \"' + $env:POSTGUY_UPDATE_MSI + '\" /passive /norestart') -Wait; \
        if ($env:POSTGUY_UPDATE_ARGS) { Start-Process -FilePath $env:POSTGUY_UPDATE_EXE -ArgumentList $env:POSTGUY_UPDATE_ARGS } \
        else { Start-Process -FilePath $env:POSTGUY_UPDATE_EXE }";
    let mut command = Command::new("powershell.exe");
    command
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .env("POSTGUY_UPDATE_PID", std::process::id().to_string())
        .env("POSTGUY_UPDATE_MSI", msi)
        .env("POSTGUY_UPDATE_EXE", exe)
        .env("POSTGUY_UPDATE_ARGS", line);
    spawn(&mut command, "starting the installer")
}

// ---- Linux ----------------------------------------------------------------------------------

fn prepare_tarball(root: &Path, package: &Path, unpacked: &Path) -> Result<PreparedUpdate, String> {
    let bin = root.join("bin");
    ensure_writable(&bin)?;
    run(Command::new("tar").arg("-xzf").arg(package).arg("-C").arg(unpacked).arg("--no-same-owner"), "unpacking the update")?;
    // The archive holds one folder, `postguy-<version>-linux-<arch>/`, with bin/ and share/.
    let top = std::fs::read_dir(unpacked)
        .map_err(|err| format!("reading {}: {err}", unpacked.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .find(|path| path.join("bin/postguy").is_file())
        .ok_or_else(|| "The downloaded archive does not contain bin/postguy. Nothing was installed.".to_string())?;
    let mut files = vec![(top.join("bin/postguy"), bin.join("postguy"))];
    if top.join("bin/postguy-cli").is_file() && bin.join("postguy-cli").is_file() {
        files.push((top.join("bin/postguy-cli"), bin.join("postguy-cli")));
    }
    Ok(PreparedUpdate { signature: unsigned_linux(), step: Step::ReplaceFiles { files, relaunch: bin.join("postguy") } })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_arguments_round_trip_through_command_line_quoting() {
        assert_eq!(quote_windows_arg("--workspace"), "--workspace");
        assert_eq!(quote_windows_arg(""), "\"\"");
        assert_eq!(quote_windows_arg(r"C:\My Work\"), r#""C:\My Work\\""#);
        assert_eq!(quote_windows_arg(r#"say "hi""#), r#""say \"hi\"""#);
        assert_eq!(quote_windows_arg(r#"a\"b c"#), r#""a\\\"b c""#);
    }

    #[test]
    fn a_signed_app_only_accepts_the_same_signer() {
        let signer = Some("CN=Postguy".to_string());
        assert!(matches!(compare_signers(&signer, &signer, "Authenticode"), Ok(SignatureCheck::Matched { .. })));
        assert!(compare_signers(&signer, &Some("CN=Someone else".into()), "Authenticode").is_err());
        assert!(compare_signers(&signer, &None, "Authenticode").is_err(), "a signed app refuses an unsigned update");
        assert!(matches!(compare_signers(&None, &None, "Authenticode"), Ok(SignatureCheck::NotChecked { .. })));
    }

    #[test]
    fn replacing_a_file_keeps_the_old_one_aside_and_restores_it_on_failure() {
        let dir = tempfile::tempdir().expect("tempdir");
        let installed = dir.path().join("postguy");
        let new = dir.path().join("download");
        std::fs::write(&installed, b"old").expect("installed");
        std::fs::write(&new, b"new").expect("new");
        let old = replace_file(&new, &installed).expect("replace");
        assert_eq!(std::fs::read(&installed).expect("read"), b"new");
        assert_eq!(std::fs::read(&old).expect("read old"), b"old");

        let missing = dir.path().join("missing-download");
        assert!(replace_file(&missing, &installed).is_err());
        assert_eq!(std::fs::read(&installed).expect("read"), b"new", "a failed copy leaves the install alone");
    }
}
