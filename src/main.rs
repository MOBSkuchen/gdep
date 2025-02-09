use std::env;
use git2::{Repository, Error, BranchType, RemoteCallbacks, Cred};
use std::process::Command;

use std::process::{Command, Stdio};
use std::thread;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

fn long_running_function(stop_flag: Arc<Mutex<bool>>, sender: mpsc::Sender<()>) {
    while !*stop_flag.lock().unwrap() {
        println!("Executing function...");
        thread::sleep(Duration::from_secs(1));  // Simulating some work.
    }

    println!("Function execution stopped. Sending signal to main thread...");
    sender.send(()).expect("Failed to send stop signal to main thread");
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

fn fetch_updates(repo: &Repository, remote_name: &str, branch_name: String) -> Result<(), Error> {
    let mut remote = repo.find_remote(remote_name)?;

    let mut cb = RemoteCallbacks::new();
    cb.credentials(|_, _, _| Cred::default()); // Use default credentials

    let mut fetch_options = git2::FetchOptions::new();
    fetch_options.remote_callbacks(cb);

    remote.fetch(&[branch_name], Some(&mut fetch_options), None)?;
    println!("Fetched latest changes from {}", remote_name);

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

fn do_update_step() {
    
}

fn execute() {
    let stop_flag = Arc::new(Mutex::new(false));
    let (tx, rx) = mpsc::channel();

    let mut child = Command::new("sleep")
        .arg("10")
        .stdout(Stdio::piped())
        .spawn()
        .expect("Failed to start subprocess");
    
    let stop_flag_clone = Arc::clone(&stop_flag);
    let function_handle = thread::spawn(move || {
        long_running_function(stop_flag_clone, tx);
    });
    
    let result = child.wait().expect("Failed to wait on child");

    // After the subprocess finishes, print the status
    if result.success() {
        println!("Subprocess completed successfully.");
    } else {
        println!("Subprocess failed with exit code: {}", result);
    }

    // Wait for a signal from the function thread to stop everything
    rx.recv().expect("Failed to receive stop signal");

    // Signal the function to stop immediately.
    *stop_flag.lock().unwrap() = true;

    // Kill the subprocess
    println!("Terminating the subprocess...");
    child.kill().expect("Failed to kill the subprocess");

    // Wait for the function thread to finish
    function_handle.join().expect("Function thread panicked");

    println!("Main thread exiting.");
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

    let us = repo_update_cycle(&repo, &branch)?;
    println!("{:?}", us);

    fetch_updates(&repo, "origin", branch)?;
    merge_main_branch(&repo)?;

    Ok(())
}