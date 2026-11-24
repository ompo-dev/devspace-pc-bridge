//! Opt-in Windows self-updater. A GitHub release must contain both the official
//! portable EXE and its .sha256. Download and validate *before* asking the old
//! executable to exit; a helper copy of the currently trusted executable then
//! waits for it, swaps files on the same volume and starts the updated version.

use crate::setup::TaskEvent;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc::Sender,
    thread,
    time::Duration,
};

const LATEST_URL: &str = "https://api.github.com/repos/ompo-dev/devspace-pc-bridge/releases/latest";
const RELEASE_BASE: &str = "https://github.com/ompo-dev/devspace-pc-bridge/releases/download";
const EXE_NAME: &str = "DevSpacePCBridge.exe";
const CHECKSUM_NAME: &str = "DevSpacePCBridge.exe.sha256";
const CHECK_INTERVAL: Duration = Duration::from_secs(4 * 3600);
const MAX_DOWNLOAD: u64 = 128 * 1024 * 1024;
const MAX_METADATA: u64 = 1024 * 1024;
const MAX_SHA: u64 = 4096;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub tag: String,
    exe_url: String,
    sha_url: String,
    size: u64,
}

#[derive(Debug)]
pub struct PreparedUpdate {
    pub tag: String,
    pub file: PathBuf,
    pub sha256: String,
}

pub fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let mut parts = text.trim().trim_start_matches('v').split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts
        .next()?
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()?;
    Some((major, minor, patch))
}

fn stable_tag(tag: &str) -> bool {
    tag.strip_prefix('v').is_some_and(|version| {
        version.split('.').count() == 3
            && version
                .split('.')
                .all(|piece| !piece.is_empty() && piece.bytes().all(|b| b.is_ascii_digit()))
            && parse_version(version).is_some()
    })
}

pub fn is_newer(current: &str, latest: &str) -> bool {
    match (parse_version(current), parse_version(latest)) {
        (Some(current), Some(latest)) => latest > current,
        _ => false,
    }
}

fn asset_url(data: &Value, tag: &str, name: &str) -> Option<(String, u64)> {
    let expected = format!("{RELEASE_BASE}/{tag}/{name}");
    data.get("assets")?.as_array()?.iter().find_map(|asset| {
        if asset.get("name")?.as_str()? == name
            && asset.get("browser_download_url")?.as_str()? == expected
        {
            Some((expected.clone(), asset.get("size")?.as_u64()?))
        } else {
            None
        }
    })
}

pub fn parse_release(data: &Value) -> Option<Release> {
    let tag = data.get("tag_name")?.as_str()?;
    if !stable_tag(tag) || data.get("draft")?.as_bool()? || data.get("prerelease")?.as_bool()? {
        return None;
    }
    let (exe_url, size) = asset_url(data, tag, EXE_NAME)?;
    let (sha_url, sha_size) = asset_url(data, tag, CHECKSUM_NAME)?;
    if !(1..=MAX_DOWNLOAD).contains(&size) || !(1..=MAX_SHA).contains(&sha_size) {
        return None;
    }
    Some(Release {
        tag: tag.to_owned(),
        exe_url,
        sha_url,
        size,
    })
}

fn client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .user_agent(concat!("DevSpacePCBridge/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(12))
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|e| format!("Cliente HTTP: {e}"))
}

fn read_bounded(mut reader: impl Read, max: u64) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    reader
        .by_ref()
        .take(max + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > max {
        return Err("Resposta do servidor maior que o limite permitido".into());
    }
    Ok(bytes)
}

fn latest_metadata(client: &reqwest::blocking::Client) -> Result<Value, String> {
    let response = client
        .get(LATEST_URL)
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|e| format!("GitHub API: {e}"))?
        .error_for_status()
        .map_err(|e| format!("GitHub API: {e}"))?;
    let bytes = read_bounded(response, MAX_METADATA)?;
    serde_json::from_slice(&bytes).map_err(|e| format!("Release JSON: {e}"))
}

fn latest_release(client: &reqwest::blocking::Client) -> Result<Release, String> {
    let data = latest_metadata(client)?;
    parse_release(&data).ok_or_else(|| {
        "Release ainda nao tem EXE e checksum oficiais (ou metadados invalidos).".into()
    })
}

pub fn spawn_check(tx: Sender<TaskEvent>) {
    if std::env::var_os("DEVSPACE_BRIDGE_NO_UPDATE_CHECK").is_some() {
        return;
    }
    thread::spawn(move || loop {
        if let Ok(client) = client() {
            if let Ok(release) = latest_release(&client) {
                if is_newer(env!("CARGO_PKG_VERSION"), &release.tag)
                    && tx.send(TaskEvent::Update(release.tag)).is_err()
                {
                    break;
                }
            }
        }
        thread::sleep(CHECK_INTERVAL);
    });
}

pub fn spawn_manual_check(tx: Sender<TaskEvent>) {
    thread::spawn(move || {
        let result = client().and_then(|http| latest_metadata(&http));
        match result {
            Ok(data) => {
                let tag = data.get("tag_name").and_then(Value::as_str).unwrap_or("");
                let message = if !stable_tag(tag) {
                    "O GitHub nao retornou uma release estavel valida.".to_owned()
                } else if !is_newer(env!("CARGO_PKG_VERSION"), tag) {
                    format!("Voce ja possui a versao mais recente (release {tag}).")
                } else if let Some(release) = parse_release(&data) {
                    let _ = tx.send(TaskEvent::Update(release.tag));
                    return;
                } else {
                    format!("A release {tag} existe, mas ainda faltam o EXE e/ou o checksum oficiais. Aguarde a conclusao do pipeline.")
                };
                let _ = tx.send(TaskEvent::Line(message));
            }
            Err(err) => {
                let _ = tx.send(TaskEvent::Line(format!(
                    "Nao foi possivel verificar atualizacoes: {err}"
                )));
            }
        }
    });
}

pub fn parse_checksum(text: &str) -> Option<String> {
    let mut parts = text.split_whitespace();
    let hash = parts.next()?;
    let name = parts.next()?;
    (parts.next().is_none()
        && name == EXE_NAME
        && hash.len() == 64
        && hash.bytes().all(|b| b.is_ascii_hexdigit()))
    .then(|| hash.to_ascii_lowercase())
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|e| format!("Abrir arquivo: {e}"))?;
    let mut sha = Sha256::new();
    let mut chunk = [0u8; 65536];
    loop {
        let n = file.read(&mut chunk).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        sha.update(&chunk[..n]);
    }
    Ok(sha.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn check_hash(path: &Path, expected: &str) -> Result<(), String> {
    if sha256_file(path)? != expected {
        return Err("SHA-256 do executavel nao corresponde ao checksum da release.".into());
    }
    Ok(())
}

fn unique_file(dir: &Path, purpose: &str) -> PathBuf {
    let mut nonce = [0u8; 8];
    getrandom::fill(&mut nonce).expect("Falha obtendo aleatoriedade para o updater");
    let suffix: String = nonce.iter().map(|n| format!("{n:02x}")).collect();
    dir.join(format!("DevSpacePCBridge.{purpose}-{suffix}.exe"))
}

fn target_exe() -> Result<PathBuf, String> {
    let path = std::env::current_exe().map_err(|e| e.to_string())?;
    if path
        .file_name()
        .and_then(|n| n.to_str())
        .is_none_or(|name| !name.eq_ignore_ascii_case(EXE_NAME))
    {
        return Err("Atualizacao disponivel apenas para DevSpacePCBridge.exe instalado/portatil; nao para binarios renomeados.".into());
    }
    Ok(path)
}

fn prepare(tag: &str, tx: &Sender<TaskEvent>) -> Result<PreparedUpdate, String> {
    let target = target_exe()?;
    let parent = target.parent().ok_or("Executavel sem diretorio")?;
    let http = client()?;
    let release = latest_release(&http)?;
    if release.tag != tag || !is_newer(env!("CARGO_PKG_VERSION"), &release.tag) {
        return Err(
            "A release mudou desde a confirmacao. Verifique novamente antes de atualizar.".into(),
        );
    }
    let _ = tx.send(TaskEvent::Line(format!(
        "[10%] Consultando checksum da release {}...",
        release.tag
    )));
    let checksum = http
        .get(&release.sha_url)
        .send()
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;
    let sha_bytes = read_bounded(checksum, MAX_SHA)?;
    let expected = parse_checksum(std::str::from_utf8(&sha_bytes).map_err(|e| e.to_string())?)
        .ok_or("Checksum .sha256 invalido; esperado SHA256 + DevSpacePCBridge.exe")?;
    let staged = unique_file(parent, "incoming");
    let result = (|| -> Result<(), String> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staged)
            .map_err(|e| format!("Nao foi possivel gravar ao lado do executavel: {e}"))?;
        let mut response = http
            .get(&release.exe_url)
            .send()
            .map_err(|e| format!("Download: {e}"))?
            .error_for_status()
            .map_err(|e| format!("Download: {e}"))?;
        let mut digest = Sha256::new();
        let mut done = 0u64;
        let mut percent = 0u64;
        let mut buffer = [0u8; 65536];
        loop {
            let len = response
                .read(&mut buffer)
                .map_err(|e| format!("Download interrompido: {e}"))?;
            if len == 0 {
                break;
            }
            done = done
                .checked_add(len as u64)
                .ok_or("Download muito grande")?;
            if done > MAX_DOWNLOAD || done > release.size {
                return Err("Executavel excedeu o tamanho anunciado".into());
            }
            file.write_all(&buffer[..len])
                .map_err(|e| format!("Gravando download: {e}"))?;
            digest.update(&buffer[..len]);
            let now = done.saturating_mul(75) / release.size + 15;
            if now >= percent + 5 {
                percent = now;
                let _ = tx.send(TaskEvent::Line(format!(
                    "[{now}%] Baixados {done} de {} bytes",
                    release.size
                )));
            }
        }
        file.sync_all()
            .map_err(|e| format!("Sincronizando arquivo: {e}"))?;
        if done != release.size {
            return Err("Download incompleto: tamanho diferente do publicado.".into());
        }
        let actual: String = digest
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        if actual != expected {
            return Err("Download rejeitado: SHA-256 incorreto.".into());
        }
        let _ = tx.send(TaskEvent::Line(
            "[100%] Arquivo verificado. Preparando substituicao segura...".into(),
        ));
        Ok(())
    })();
    if let Err(err) = result {
        let _ = fs::remove_file(&staged);
        return Err(err);
    }
    Ok(PreparedUpdate {
        tag: release.tag,
        file: staged,
        sha256: expected,
    })
}

pub fn spawn_prepare(tag: String, tx: Sender<TaskEvent>) {
    thread::spawn(move || match prepare(&tag, &tx) {
        Ok(update) => {
            let _ = tx.send(TaskEvent::UpdateReady(update));
        }
        Err(err) => {
            let _ = tx.send(TaskEvent::UpdateFailed(err));
        }
    });
}

#[cfg(windows)]
pub fn launch_helper(prepared: &PreparedUpdate) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let target = target_exe()?;
    if target.parent() != prepared.file.parent() {
        return Err("Atualizacao deve estar no mesmo diretorio do EXE.".into());
    }
    check_hash(&prepared.file, &prepared.sha256)?;
    let helper = unique_file(target.parent().ok_or("Sem diretorio")?, "updater");
    fs::copy(&target, &helper).map_err(|e| format!("Criando helper de atualizacao: {e}"))?;
    let result = Command::new(&helper)
        .args(["--apply-update", &std::process::id().to_string()])
        .arg(&target)
        .arg(&prepared.file)
        .arg(&prepared.sha256)
        .arg(&prepared.tag)
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .spawn()
        .map_err(|e| format!("Iniciando updater: {e}"));
    match result {
        Ok(_child) => Ok(()),
        Err(err) => {
            let _ = fs::remove_file(helper);
            Err(err)
        }
    }
}

/// Does not run as part of the TUI; the separate helper waits until the running
/// version has closed and restores the original on any failed swap/start.
#[cfg(windows)]
pub fn apply_update(args: &[String]) -> i32 {
    use std::os::windows::process::CommandExt;
    let run = (|| -> Result<(), String> {
        if args.len() != 5 {
            return Err(
                "Uso interno: --apply-update <pid> <original> <incoming> <sha> <tag>".into(),
            );
        }
        let pid: u32 = args[0].parse().map_err(|_| "PID invalido")?;
        let target = PathBuf::from(&args[1]);
        let incoming = PathBuf::from(&args[2]);
        let expected = &args[3];
        let tag = &args[4];
        if !stable_tag(tag)
            || !is_newer(env!("CARGO_PKG_VERSION"), tag)
            || expected.len() != 64
            || !expected.bytes().all(|b| b.is_ascii_hexdigit())
            || target
                .file_name()
                .and_then(|x| x.to_str())
                .is_none_or(|x| !x.eq_ignore_ascii_case(EXE_NAME))
            || target.parent() != incoming.parent()
            || !incoming
                .file_name()
                .and_then(|x| x.to_str())
                .is_some_and(|name| {
                    name.starts_with("DevSpacePCBridge.incoming-") && name.ends_with(".exe")
                })
        {
            return Err("Argumentos do atualizador invalidos.".into());
        }
        wait_for_process(pid)?;
        check_hash(&incoming, expected)?;
        let parent = target.parent().ok_or("Sem diretorio")?;
        let backup = unique_file(parent, "backup");
        swap_files(&target, &incoming, &backup, expected)?;
        // Record success *before* spawning: the new process may read this
        // message immediately during startup.
        let _ = record_outcome(&format!(
            "Atualizado para {tag}. Copia anterior preservada: {}",
            backup.display()
        ));
        match Command::new(&target).creation_flags(0x0000_0010).spawn() {
            // CREATE_NEW_CONSOLE
            Ok(_) => Ok(()),
            Err(err) => {
                let _ = fs::remove_file(&target);
                fs::rename(&backup, &target).map_err(|restore| {
                    format!(
                        "Nova versao falhou ({err}); restaure manualmente o backup {}: {restore}",
                        backup.display()
                    )
                })?;
                Err(format!("Nao iniciou a nova versao: {err}"))
            }
        }
    })();
    match run {
        Ok(()) => 0,
        Err(err) => {
            let _ = record_outcome(&format!("Atualizacao falhou: {err}"));
            eprintln!("Updater: {err}");
            // The old TUI already exited after launching this helper. Reopen
            // whichever verified/rolled-back EXE still exists, so an AV lock
            // or a failed rename does not strand the user without an app.
            if let (Some(target), true) = (args.get(1), args.len() == 5) {
                let path = Path::new(target);
                if path.is_file()
                    && path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.eq_ignore_ascii_case(EXE_NAME))
                {
                    let _ = Command::new(path).creation_flags(0x0000_0010).spawn();
                }
            }
            1
        }
    }
}

#[cfg(windows)]
fn wait_for_process(pid: u32) -> Result<(), String> {
    use windows::Win32::{
        Foundation::{CloseHandle, WAIT_OBJECT_0},
        System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE},
    };
    let handle = match unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) } {
        Ok(handle) => handle,
        // The TUI can exit between spawning us and opening its PID.
        Err(err) if err.code().0 as u32 == 0x8007_0057 => return Ok(()),
        Err(err) => return Err(format!("Esperando encerramento do aplicativo: {err}")),
    };
    let code = unsafe { WaitForSingleObject(handle, 120_000) };
    unsafe {
        let _ = CloseHandle(handle);
    }
    if code != WAIT_OBJECT_0 {
        return Err("O aplicativo nao encerrou dentro do prazo; arquivos preservados.".into());
    }
    Ok(())
}

/// Same-directory renames keep the swap atomic per file. If anything goes
/// wrong, restore the old binary rather than leaving a broken installation.
fn swap_files(target: &Path, incoming: &Path, backup: &Path, expected: &str) -> Result<(), String> {
    check_hash(incoming, expected)?;
    if backup.exists() {
        return Err("Backup ja existe; operacao cancelada.".into());
    }
    fs::rename(target, backup).map_err(|e| format!("Salvando versao anterior: {e}"))?;
    let attempt = (|| -> Result<(), String> {
        fs::rename(incoming, target).map_err(|e| format!("Substituindo EXE: {e}"))?;
        check_hash(target, expected)
    })();
    if let Err(err) = attempt {
        let _ = fs::remove_file(target);
        fs::rename(backup, target)
            .map_err(|e| format!("{err}; ERRO CRITICO restaurando backup: {e}"))?;
        return Err(err);
    }
    Ok(())
}

fn outcome_file() -> PathBuf {
    crate::storage::settings_path().with_file_name("last-update.json")
}

fn record_outcome(message: &str) -> Result<(), String> {
    let path = outcome_file();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::write(path, json!({"message": message}).to_string()).map_err(|e| e.to_string())
}

pub fn take_outcome() -> Option<String> {
    let path = outcome_file();
    let bytes = fs::read(&path).ok()?;
    if bytes.len() > 4096 {
        return None;
    }
    let message = serde_json::from_slice::<Value>(&bytes)
        .ok()?
        .get("message")?
        .as_str()?
        .to_owned();
    let _ = fs::remove_file(path);
    Some(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(tag: &str, include_hash: bool) -> Value {
        let exe = |name: &str, size| json!({"name":name,"size":size,"browser_download_url":format!("{RELEASE_BASE}/{tag}/{name}")});
        let mut assets = vec![exe(EXE_NAME, 5120)];
        if include_hash {
            assets.push(exe(CHECKSUM_NAME, 92));
        }
        json!({"tag_name":tag,"draft":false,"prerelease":false,"assets":assets})
    }
    #[test]
    fn version_comparison() {
        assert_eq!(parse_version("v0.4.1"), Some((0, 4, 1)));
        assert_eq!(parse_version("1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("v2.0.0-beta.1"), Some((2, 0, 0)));
        assert_eq!(parse_version("nightly"), None);
        assert_eq!(parse_version("1.2"), None);
        assert!(is_newer("0.5.0", "v0.6.0"));
        assert!(!is_newer("0.5.0", "v0.4.0"));
    }
    #[test]
    fn release_requires_both_official_assets() {
        assert!(parse_release(&fixture("v0.6.0", true)).is_some());
        assert!(parse_release(&fixture("v0.4.0", false)).is_none());
        assert!(parse_release(&fixture("v0.6.0-rc1", true)).is_none());
        let mut wrong = fixture("v0.6.0", true);
        wrong["assets"][0]["browser_download_url"] = json!("https://malicious.example/steal.exe");
        assert!(parse_release(&wrong).is_none());
    }
    #[test]
    fn checksum_accepts_only_named_exe_and_hex() {
        let hash = "ab".repeat(32);
        assert_eq!(
            parse_checksum(&format!("{hash}  DevSpacePCBridge.exe\n")),
            Some(hash.clone())
        );
        assert!(parse_checksum(&format!("{hash}  malware.exe")).is_none());
        assert!(parse_checksum("garbage").is_none());
    }
    #[test]
    fn failed_hash_never_modifies_original() {
        let dir = std::env::temp_dir().join(format!("update-test-{}", crate::clock::now_nanos()));
        fs::create_dir(&dir).unwrap();
        let old = dir.join(EXE_NAME);
        let next = dir.join("incoming.exe");
        let backup = dir.join("backup.exe");
        fs::write(&old, b"OLD").unwrap();
        fs::write(&next, b"NEW").unwrap();
        assert!(swap_files(&old, &next, &backup, &"0".repeat(64)).is_err());
        assert_eq!(fs::read(&old).unwrap(), b"OLD");
        assert!(!backup.exists());
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn verified_swap_preserves_backup() {
        let dir = std::env::temp_dir().join(format!("update-test-{}", crate::clock::now_nanos()));
        fs::create_dir(&dir).unwrap();
        let old = dir.join(EXE_NAME);
        let next = dir.join("incoming.exe");
        let backup = dir.join("backup.exe");
        fs::write(&old, b"OLD").unwrap();
        fs::write(&next, b"NEW").unwrap();
        let sha = sha256_file(&next).unwrap();
        swap_files(&old, &next, &backup, &sha).unwrap();
        assert_eq!(fs::read(&old).unwrap(), b"NEW");
        assert_eq!(fs::read(&backup).unwrap(), b"OLD");
        assert!(!next.exists());
        fs::remove_dir_all(dir).unwrap();
    }
}
