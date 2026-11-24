//! Git checkpoints: a restore point to take before a chat is allowed to edit files.
//!
//! A snapshot is a commit of the current working tree (untracked files included,
//! `.gitignore` respected) built with a *temporary* index, so the real index, the
//! working tree, the current branch and `git stash` are never touched. It is kept
//! under `refs/devspace-checkpoints/`, which no branch listing shows.

use crate::{clock, setup::hidden_process};
use std::{
    cmp::Reverse,
    fs,
    path::{Path, PathBuf},
};

pub const KEEP: usize = 20;
const REF_PREFIX: &str = "refs/devspace-checkpoints/";
const GIT: &str = if cfg!(windows) { "git.exe" } else { "git" };

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checkpoint {
    pub name: String,
    pub short_id: String,
    pub created: u64,
}

fn git(repo: &Path, args: &[&str], envs: &[(&str, &str)]) -> Result<String, String> {
    let mut command = hidden_process(GIT);
    command.arg("-C").arg(repo).args(args);
    for (key, value) in envs {
        command.env(key, value);
    }
    let output = command
        .output()
        .map_err(|e| format!("Não consegui executar o git: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    } else {
        Err(format!(
            "git {}: {}",
            args.first().copied().unwrap_or_default(),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

fn snapshot_commit(repo: &Path, index: &Path) -> Result<String, String> {
    let index_text = index.to_string_lossy();
    let index_env = [("GIT_INDEX_FILE", index_text.as_ref())];
    git(repo, &["add", "-A"], &index_env)?;
    let tree = git(repo, &["write-tree"], &index_env)?;
    let parent = git(repo, &["rev-parse", "-q", "--verify", "HEAD"], &[]).ok();
    let message = format!(
        "DevSpace checkpoint {}",
        clock::format_utc(clock::now_secs())
    );
    let mut args = vec!["commit-tree", "-m", message.as_str()];
    if let Some(parent) = parent.as_deref() {
        args.extend(["-p", parent]);
    }
    args.push(tree.as_str());
    let identity = [
        ("GIT_AUTHOR_NAME", "DevSpace PC Bridge"),
        ("GIT_AUTHOR_EMAIL", "bridge@localhost"),
        ("GIT_COMMITTER_NAME", "DevSpace PC Bridge"),
        ("GIT_COMMITTER_EMAIL", "bridge@localhost"),
    ];
    git(repo, &args, &identity)
}

fn unique_ref(repo: &Path) -> Result<String, String> {
    let epoch = clock::now_secs();
    for attempt in 1..=1000 {
        let name = if attempt == 1 {
            format!("{REF_PREFIX}{epoch}")
        } else {
            format!("{REF_PREFIX}{epoch}-{attempt}")
        };
        if git(repo, &["show-ref", "--verify", "--quiet", &name], &[]).is_err() {
            return Ok(name);
        }
    }
    Err("Checkpoints demais no mesmo segundo".into())
}

pub fn create(repo: &Path) -> Result<Checkpoint, String> {
    let git_dir = PathBuf::from(git(repo, &["rev-parse", "--absolute-git-dir"], &[])?);
    let index = git_dir.join(format!(
        "devspace-checkpoint-{}-{}.index",
        std::process::id(),
        clock::now_nanos()
    ));
    let commit = snapshot_commit(repo, &index);
    // Nothing else may keep a stray index around, whatever happened above.
    let _ = fs::remove_file(&index);
    let commit = commit?;
    let name = unique_ref(repo)?;
    git(repo, &["update-ref", &name, &commit], &[])?;
    let short_id = git(repo, &["rev-parse", "--short", &commit], &[])?;
    // Pruning is housekeeping; a failure must not hide that the snapshot exists.
    let _ = prune(repo, KEEP);
    Ok(Checkpoint {
        name,
        short_id,
        created: clock::now_secs(),
    })
}

fn sort_key(name: &str) -> (u64, u32) {
    let rest = name.strip_prefix(REF_PREFIX).unwrap_or(name);
    match rest.split_once('-') {
        Some((epoch, attempt)) => (epoch.parse().unwrap_or(0), attempt.parse().unwrap_or(1)),
        None => (rest.parse().unwrap_or(0), 1),
    }
}

fn parse_line(line: &str) -> Option<Checkpoint> {
    let mut parts = line.split('|');
    Some(Checkpoint {
        name: parts.next()?.to_owned(),
        short_id: parts.next()?.to_owned(),
        created: parts.next()?.trim().parse().ok()?,
    })
}

/// Newest first.
pub fn list(repo: &Path) -> Result<Vec<Checkpoint>, String> {
    let output = git(
        repo,
        &[
            "for-each-ref",
            "--format=%(refname)|%(objectname:short)|%(committerdate:unix)",
            REF_PREFIX,
        ],
        &[],
    )?;
    let mut items: Vec<Checkpoint> = output.lines().filter_map(parse_line).collect();
    items.sort_by_key(|item| Reverse(sort_key(&item.name)));
    Ok(items)
}

fn prune(repo: &Path, keep: usize) -> Result<(), String> {
    for old in list(repo)?.into_iter().skip(keep) {
        git(repo, &["update-ref", "-d", &old.name], &[])?;
    }
    Ok(())
}

/// Puts the files of a checkpoint back. Files created after it are left alone,
/// and the current state is checkpointed first so a restore can be undone too.
pub fn restore(repo: &Path, name: &str) -> Result<(), String> {
    if !name.starts_with(REF_PREFIX) {
        return Err("Nome de checkpoint inválido".into());
    }
    // Resolve to a commit id now: creating the safety checkpoint may prune this ref.
    let commit = git(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{name}^{{commit}}"),
        ],
        &[],
    )
    .map_err(|_| "Checkpoint não encontrado".to_owned())?;
    create(repo).map_err(|e| format!("Não salvei o estado atual antes de restaurar: {e}"))?;
    git(
        repo,
        &[
            "restore",
            &format!("--source={commit}"),
            "--worktree",
            "--",
            ".",
        ],
        &[],
    )?;
    Ok(())
}

pub fn find_repos(root: &Path) -> Vec<PathBuf> {
    if root.join(".git").exists() {
        return vec![root.to_path_buf()];
    }
    let mut repos: Vec<PathBuf> = fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.join(".git").exists())
        .collect();
    repos.sort();
    repos
}

pub fn snapshot_all(root: &Path, emit: &mut dyn FnMut(String)) -> Result<(), String> {
    let repos = find_repos(root);
    if repos.is_empty() {
        return Err(format!("Nenhum repositório git em {}", root.display()));
    }
    let mut failed = 0;
    for repo in &repos {
        let label = repo.file_name().map_or_else(
            || repo.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        match create(repo) {
            Ok(checkpoint) => emit(format!("OK {label}: {}", checkpoint.short_id)),
            Err(err) => {
                failed += 1;
                emit(format!("ERRO {label}: {err}"));
            }
        }
    }
    if failed == 0 {
        Ok(())
    } else {
        Err(format!("{failed} de {} repositórios falharam", repos.len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git_available() -> bool {
        Command::new(GIT)
            .arg("--version")
            .output()
            .is_ok_and(|output| output.status.success())
    }

    fn scratch_repo(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "bridge-checkpoint-{label}-{}-{}",
            std::process::id(),
            clock::now_nanos()
        ));
        fs::create_dir_all(&dir).expect("scratch repo dir");
        git(&dir, &["init", "-q"], &[]).expect("git init");
        fs::write(dir.join("a.txt"), "one").unwrap();
        git(&dir, &["add", "-A"], &[]).unwrap();
        git(
            &dir,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "-m",
                "init",
            ],
            &[],
        )
        .expect("initial commit");
        dir
    }

    #[test]
    fn checkpoint_captures_untracked_files_and_restores_them() {
        if !git_available() {
            return;
        }
        let repo = scratch_repo("roundtrip");
        fs::write(repo.join("a.txt"), "two").unwrap();
        fs::write(repo.join("b.txt"), "new").unwrap();
        let checkpoint = create(&repo).expect("create");
        assert!(checkpoint.name.starts_with(REF_PREFIX));

        fs::write(repo.join("a.txt"), "three").unwrap();
        fs::remove_file(repo.join("b.txt")).unwrap();
        restore(&repo, &checkpoint.name).expect("restore");

        assert_eq!(fs::read_to_string(repo.join("a.txt")).unwrap(), "two");
        assert_eq!(fs::read_to_string(repo.join("b.txt")).unwrap(), "new");
        // The restore saved the state it replaced, so it can be undone.
        assert!(list(&repo).unwrap().len() >= 2);
        fs::remove_dir_all(repo).ok();
    }
    #[test]
    fn creating_a_checkpoint_leaves_the_repo_untouched() {
        if !git_available() {
            return;
        }
        let repo = scratch_repo("untouched");
        fs::write(repo.join("a.txt"), "edited").unwrap();
        fs::write(repo.join("untracked.txt"), "x").unwrap();
        let status_before = git(&repo, &["status", "--porcelain"], &[]).unwrap();
        let branch_before = git(&repo, &["rev-parse", "HEAD"], &[]).unwrap();
        create(&repo).expect("create");
        assert_eq!(
            git(&repo, &["status", "--porcelain"], &[]).unwrap(),
            status_before
        );
        assert_eq!(
            git(&repo, &["rev-parse", "HEAD"], &[]).unwrap(),
            branch_before
        );
        // Nothing was staged by the snapshot.
        assert!(git(&repo, &["diff", "--cached", "--quiet"], &[]).is_ok());
        let leftovers = fs::read_dir(repo.join(".git"))
            .unwrap()
            .flatten()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("devspace-checkpoint-")
            })
            .count();
        assert_eq!(leftovers, 0);
        fs::remove_dir_all(repo).ok();
    }
    #[test]
    fn only_the_newest_checkpoints_are_kept() {
        if !git_available() {
            return;
        }
        let repo = scratch_repo("prune");
        for round in 0..4 {
            fs::write(repo.join("a.txt"), format!("v{round}")).unwrap();
            create(&repo).expect("create");
        }
        assert_eq!(list(&repo).unwrap().len(), 4);
        prune(&repo, 2).expect("prune");
        let kept = list(&repo).unwrap();
        assert_eq!(kept.len(), 2);
        assert!(sort_key(&kept[0].name) >= sort_key(&kept[1].name));
        fs::remove_dir_all(repo).ok();
    }
    #[test]
    fn restore_refuses_foreign_refs() {
        let repo = std::env::temp_dir();
        assert!(restore(&repo, "refs/heads/main").is_err());
        assert!(restore(&repo, "HEAD").is_err());
    }
    #[test]
    fn repositories_are_found_one_level_down() {
        let root = std::env::temp_dir().join(format!("bridge-repos-{}", clock::now_nanos()));
        fs::create_dir_all(root.join("one").join(".git")).unwrap();
        fs::create_dir_all(root.join("two")).unwrap();
        fs::create_dir_all(root.join("three").join(".git")).unwrap();
        let found = find_repos(&root);
        let names: Vec<_> = found
            .iter()
            .filter_map(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["one", "three"]);
        fs::remove_dir_all(root).ok();
    }
    #[test]
    fn ref_names_sort_numerically() {
        assert!(
            sort_key("refs/devspace-checkpoints/100-10")
                > sort_key("refs/devspace-checkpoints/100-2")
        );
        assert!(
            sort_key("refs/devspace-checkpoints/101") > sort_key("refs/devspace-checkpoints/100-9")
        );
    }
}
