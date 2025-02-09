mod config;

use std::env;
use git2::{Repository, Error, BranchType, RemoteCallbacks, Cred};
use std::process::{Command, Stdio};
use std::thread;
use std::sync::{Arc, Mutex, mpsc};
use crate::config::Config;

fn update_sync(repo_path: Arc<String>, branch_name: Arc<String>, stop_flag: Arc<Mutex<bool>>, sender: mpsc::Sender<bool>) {
    let mut err = false;
    let repo_x = Repository::open(&*repo_path);
    
    if repo_x.is_ok() {
        let repo = repo_x.unwrap();
        while !*stop_flag.lock().unwrap() {
            let res = repo_update_cycle(&repo, &branch_name);
            if res.is_err() {
                err = true;
                break
            }

            let urs = res.unwrap();
            match urs {
                UpdateRelationState::Up2Date => {
                    continue
                }
                UpdateRelationState::Ahead(a) => {
                    err = true;
                    if err {
                        println!("Repo is {a} ahead of the remote repo")
                    }
                    break
                }
                UpdateRelationState::Behind(_) => {
                    err = update_repo(&repo, &branch_name).is_err();
                    if err {
                        println!("Failed to update repo!")
                    }
                    break
                }
                UpdateRelationState::AheadBehind(a, b) => {
                    if err {
                        println!("Repo is {a} ahead of the remote repo and {b} behind")
                    }
                }
            }
        }
    }

    println!("Function execution stopped. Sending signal to main thread...");
    sender.send(err).expect("Failed to send stop signal to main thread");
}

#[derive(Debug)]
pub enum UpdateRelationState {
    Up2Date,
    Ahead(usize),
    Behind(usize),
    AheadBehind(usize, usize)
}

pub enum UpdateLog {
    AlreadyUp2Date,
    MergeConflicts,
    Success
}

fn merge_main_branch(repo: &Repository) -> Result<UpdateLog, Error> {
    let fetch_head = repo.find_reference("FETCH_HEAD")?.peel_to_commit()?;
    let head = repo.head()?.peel_to_commit()?;

    let merge_base = repo.merge_base(head.id(), fetch_head.id())?;
    if merge_base == fetch_head.id() {
        return Ok(UpdateLog::AlreadyUp2Date);
    }

    let mut index = repo.merge_commits(&head, &fetch_head, None)?;
    if index.has_conflicts() {
        return Ok(UpdateLog::MergeConflicts);
    }

    let tree = repo.find_tree(index.write_tree()?)?;
    let sig = repo.signature()?;
    repo.commit(Some("HEAD"), &sig, &sig, "Merged changes", &tree, &[&head, &fetch_head])?;

    Ok(UpdateLog::Success)
}

fn fetch_updates(repo: &Repository, remote_name: &str, branch_name: &String) -> Result<(), Error> {
    let mut remote = repo.find_remote(remote_name)?;

    let mut cb = RemoteCallbacks::new();
    cb.credentials(|_, _, _| Cred::default()); // Use default credentials

    let mut fetch_options = git2::FetchOptions::new();
    fetch_options.remote_callbacks(cb);

    remote.fetch(&[branch_name], Some(&mut fetch_options), None)?;

    Ok(())
}

fn get_default_branch(repo: &Repository) -> Result<String, Error> {
    let branches = repo.branches(Some(BranchType::Remote))?;

    // Look for "origin/main" or "origin/master"
    let mut found_branch = None;
    for branch in branches {
        let (branch, _) = branch?;
        if let Some(name) = branch.name()? {
            if name.ends_with("/main") || name.ends_with("/master") {
                found_branch = Some(name.split('/').last().unwrap().to_string());
                break;
            }
        }
    }

    found_branch.ok_or_else(|| Error::from_str("No main/master branch found"))
}

fn repo_update_cycle(repo: &Repository, branch: &String) -> Result<UpdateRelationState, Error> {
    let head = repo.head()?.peel_to_commit()?;

    let remote_branch = repo.find_reference(format!("refs/remotes/origin/{}", branch).as_str())?.peel_to_commit()?;

    let ahead_behind = repo.graph_ahead_behind(head.id(), remote_branch.id())?;

    Ok(match ahead_behind {
        (0, 0) => UpdateRelationState::Up2Date,
        (ahead, 0) => UpdateRelationState::Ahead(ahead),
        (0, behind) => UpdateRelationState::Behind(behind),
        (ahead, behind) => UpdateRelationState::AheadBehind(ahead, behind),
    })
}

fn execute(config: Config, repo_path: String, branch_name: String) {
    let do_rerun = config.re_run;
    
    let stop_flag = Arc::new(Mutex::new(false));
    let (tx, rx) = mpsc::channel();

    let repo_path_arc = Arc::new(repo_path.clone());
    let branch_name_arc = Arc::new(branch_name.clone());

    let mut child = Command::new("sleep")
        .arg("4")
        .stdout(Stdio::piped())
        .spawn()
        .expect("Failed to start subprocess");
    
    let stop_flag_clone = Arc::clone(&stop_flag);
    let function_handle = thread::spawn(move || {
        update_sync(repo_path_arc, branch_name_arc, stop_flag_clone, tx);
    });
    
    let result = child.wait().expect("Failed to wait on child");

    if result.success() {
        println!("Running script completed successfully.");
    } else {
        println!("Running script failed with exit code: {}", result);
    }

    *stop_flag.lock().unwrap() = true;

    let err = rx.recv().expect("Failed to receive stop signal");
    if err {
        println!("Got an error while looking for updates!")
    }

    println!("Terminating the subprocess...");
    child.kill().expect("Failed to kill the subprocess");
    function_handle.join().expect("Function thread panicked");

    if do_rerun {
        execute(config, repo_path, branch_name);
    }
}

fn update_repo(repo: &Repository, branch_name: &String) -> Result<(), Error> {
    fetch_updates(repo, "origin", branch_name)?;
    merge_main_branch(repo)?;
    Ok(())
}

fn main() -> Result<(), Error> {
    let repo_url = env::args().nth(1).expect("Usage: gdep <repo_url> <repo_path>");
    let repo_path = env::args().nth(2).expect("Usage: gdep <repo_url> <repo_path>");

    let repo = match Repository::open(&repo_path) {
        Ok(repo) => repo,
        Err(_) => {
            println!("Repository not found (under '{}'), cloning...", &repo_path);
            Repository::clone(&repo_url, &repo_path)?
        }
    };

    let branch = Some(get_default_branch(&repo)?).unwrap();

    // execute(repo_path, branch);

    Ok(())
}