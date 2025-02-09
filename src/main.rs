use std::env;
use git2::{Repository, Error, BranchType};

#[derive(Debug)]
pub enum UpdateState {
    Up2Date,
    Ahead(usize),
    Behind(usize),
    AheadBehind(usize, usize)
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

fn repo_update_cycle(repo: &Repository, default_branch: Option<String>) -> Result<UpdateState, Error> {
    let head = repo.head()?.peel_to_commit()?;

    let default_branch = default_branch.or(Some(get_default_branch(&repo)?)).unwrap();
    let remote_branch = repo.find_reference(format!("refs/remotes/origin/{}", default_branch).as_str())?.peel_to_commit()?;

    let ahead_behind = repo.graph_ahead_behind(head.id(), remote_branch.id())?;

    Ok(match ahead_behind {
        (0, 0) => UpdateState::Up2Date,
        (ahead, 0) => UpdateState::Ahead(ahead),
        (0, behind) => UpdateState::Behind(behind),
        (ahead, behind) => UpdateState::AheadBehind(ahead, behind),
    })
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
    
    let us = repo_update_cycle(&repo, None)?;
    println!("{:?}", us);

    Ok(())
}