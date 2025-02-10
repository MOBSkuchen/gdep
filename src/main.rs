mod config;
mod errors;

use std::{env, io};
use std::io::Write;
use std::path::PathBuf;
use std::process::{ExitStatus};
use git2::{Error, Repository, BranchType, RemoteCallbacks, Cred, Commit, ObjectType, MergeOptions, AnnotatedCommit, FetchOptions, AutotagOption};
use std::string::ToString;
use std::thread;
use std::sync::{Arc, Mutex, mpsc};
use clap::{Arg, ArgMatches, ColorChoice};
use run_script::ScriptOptions;
use run_script::types::IoOptions;
use crate::config::{Config, ConfigError, RepoLike};
use crate::errors::GdepError;
use crate::errors::GdepError::{UpdateErrorAheadBehind, UpdateErrorRepoAhead, UpdateFailed};

pub const NAME: &str = env!("CARGO_PKG_NAME");
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const DESCRIPTION: &str = env!("CARGO_PKG_DESCRIPTION");
static DEFAULT_REPO_PATH: &str = "gdep_used_repo";

#[macro_export]
macro_rules! conv_err {
    ($pre:expr, $err: expr) => {
        $pre.or_else(|_| { Err($err) })
    };
}

#[macro_export]
macro_rules! conv_err_e {
    ($pre:expr, $err: expr) => {
        $pre.or_else(|e| { Err($err(e.to_string())) })
    };
}

fn update_sync(repo_path: Arc<String>, branch_name: Arc<String>, stop_flag: Arc<Mutex<bool>>, sender: mpsc::Sender<(Option<GdepError>, bool)>) {
    let mut err = None;
    let repo_x = Repository::open(&*repo_path);
    
    if repo_x.is_ok() {
        let repo = repo_x.unwrap();
        while !*stop_flag.lock().unwrap() {
            sender.send((None, false)).expect("Failed to send alive signal to main thread");

            let res = repo_update_cycle(&repo, &branch_name);
            if res.is_err() {
                err = Some(GdepError::from(res.unwrap_err()));
                break
            }

            let urs = res.unwrap();
            match urs {
                UpdateRelationState::Up2Date => { continue }
                UpdateRelationState::Ahead(a) => {
                    err = Some(UpdateErrorRepoAhead(a));
                    break
                }
                UpdateRelationState::Behind(_) => {
                    let tmp_err = update_repo(&repo, &*branch_name);
                    if tmp_err.is_err() {
                        let unw_err = tmp_err.unwrap_err();
                        err = Some(UpdateFailed(unw_err.to_string(), unw_err.code()))
                    } else {
                        println!("Successfully updated local repo")
                    }
                    break
                }
                UpdateRelationState::AheadBehind(a, b) => {
                    err = Some(UpdateErrorAheadBehind(a, b));
                    break
                }
            }
        }
    }

    if err.is_some() {
        println!("Error while searching for updates!")
    }
    sender.send((err, true)).expect("Failed to send stop signal to main thread");
}

#[derive(Debug)]
pub enum UpdateRelationState {
    Up2Date,
    Ahead(usize),
    Behind(usize),
    AheadBehind(usize, usize)
}

pub fn update_repo(repo: &Repository, branch_name: &str) -> Result<(), Error> {
    let remote_name = "origin";
    let mut remote = repo.find_remote(remote_name)?;
    let fetch_commit = fetch_updates(repo, &[branch_name], &mut remote)?;
    merge_updates(repo, branch_name, fetch_commit)
}

fn fetch_updates<'a>(
    repo: &'a Repository,
    refs: &[&str],
    remote: &'a mut git2::Remote,
) -> Result<AnnotatedCommit<'a>, Error> {
    let mut fo = FetchOptions::new();
    fo.download_tags(AutotagOption::All);
    remote.fetch(refs, Some(&mut fo), None)?;

    let fetch_head = repo.find_reference("FETCH_HEAD")?;
    Ok(repo.reference_to_annotated_commit(&fetch_head)?)
}

fn merge_updates(
    repo: &Repository,
    remote_branch: &str,
    fetch_commit: AnnotatedCommit,
) -> Result<(), Error> {
    let analysis = repo.merge_analysis(&[&fetch_commit])?;
    if analysis.0.is_fast_forward() {
        let refname = format!("refs/heads/{}", remote_branch);
        match repo.find_reference(&refname) {
            Ok(mut reference) => {
                reference.set_target(fetch_commit.id(), "Fast-forward")?;
                repo.set_head(&refname)?;
                repo.checkout_head(Some(git2::build::CheckoutBuilder::default().force()))?;
            }
            Err(_) => {
                repo.reference(&refname, fetch_commit.id(), true, "Setting new branch")?;
                repo.set_head(&refname)?;
                repo.checkout_head(Some(git2::build::CheckoutBuilder::default().force()))?;
            }
        }
    } else if analysis.0.is_normal() {
        let head_commit = repo.reference_to_annotated_commit(&repo.head()?)?;
        perform_merge(repo, &head_commit, &fetch_commit)?;
    }
    Ok(())
}

fn perform_merge(
    repo: &Repository,
    local: &AnnotatedCommit,
    remote: &AnnotatedCommit,
) -> Result<(), Error> {
    let local_tree = repo.find_commit(local.id())?.tree()?;
    let remote_tree = repo.find_commit(remote.id())?.tree()?;
    let ancestor_tree = repo.find_commit(repo.merge_base(local.id(), remote.id())?)?.tree()?;
    let mut index = repo.merge_trees(&ancestor_tree, &local_tree, &remote_tree, None)?;

    if index.has_conflicts() {
        println!("Merge conflicts detected...");
        repo.checkout_index(Some(&mut index), None)?;
        return Ok(());
    }

    let result_tree = repo.find_tree(index.write_tree_to(repo)?)?;
    let sig = repo.signature()?;
    let local_commit = repo.find_commit(local.id())?;
    let remote_commit = repo.find_commit(remote.id())?;
    repo.commit(Some("HEAD"), &sig, &sig, "Merge commit", &result_tree, &[&local_commit, &remote_commit])?;
    repo.checkout_head(None)?;
    Ok(())
}

fn fetch_updates2(repo: &Repository, remote_name: &str, branch_name: &String) -> Result<(), Error> {
    let mut remote = repo.find_remote(remote_name)?;

    let mut cb = RemoteCallbacks::new();
    cb.credentials(|_, _, _| Cred::default()); // Use default credentials

    let mut fetch_options = FetchOptions::new();
    fetch_options.remote_callbacks(cb);

    remote.fetch(&[branch_name], Some(&mut fetch_options), None)?;
    Ok(())
}

fn get_default_branch(repo: &Repository) -> Result<String, GdepError> {
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

    if found_branch.is_none() {
        Err(GdepError::BranchInferFailed)
    } else {
        let fb = found_branch.unwrap();
        println!("Branch inferred to be `{}`", fb);
        Ok(fb)
    }
}

fn repo_update_cycle(repo: &Repository, branch: &String) -> Result<UpdateRelationState, Error> {
    fetch_updates2(repo, "origin", branch)?;
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

fn execute(config: Config, repo_path: String, branch_name: String) -> Option<GdepError> {
    let mut do_rerun = config.re_run;
    
    let stop_flag = Arc::new(Mutex::new(false));
    let (tx, rx) = mpsc::channel();

    let repo_path_arc = Arc::new(repo_path.clone());
    let branch_name_arc = Arc::new(branch_name.clone());

    let mut options = ScriptOptions::new();
    options.working_directory = Some(PathBuf::from(&repo_path));
    options.output_redirection = IoOptions::Inherit;

    let args = vec![];

    let mut child = run_script::spawn(config.script.as_str(), &args, &options).expect("Failed to start subprocess");

    let stop_flag_clone = Arc::clone(&stop_flag);

    let update_handle = thread::spawn(move || {
        update_sync(repo_path_arc, branch_name_arc, stop_flag_clone, tx);
    });

    let mut result: Option<ExitStatus> = None;
    
    let (mut err, mut stop) = rx.recv().expect("Failed to receive singal from update thread");

    while !stop {
        let boring_result = child.try_wait();
        if boring_result.is_err() {
            *stop_flag.lock().unwrap() = true;
            break
        } else {
            result = boring_result.unwrap();
        }
        if result.is_some() {
            break;
        }
        (err, stop) = rx.recv().expect("Failed to receive singal from update thread");
    }

    if result.is_some() {
        if !result.unwrap().success() {
            println!("Running script failed with exit code: {}", result.unwrap());
            do_rerun = do_rerun && !config.exit_on_script_error;
        }
    }
    
    if err.is_some() {
        
        do_rerun = do_rerun && !config.exit_on_gdep_error;
    }

    *stop_flag.lock().unwrap() = true;

    child.kill().expect("Failed to kill the subprocess");
    update_handle.join().expect("Function thread panicked");

    if do_rerun {
        println!("Restarting...");
        execute(config, repo_path, branch_name);
    }
    
    err
}

fn load_cfg(matches: &ArgMatches, repo_path: &String) -> Result<Config, ConfigError> {
    let config_file_path = matches.get_one::<String>("config-file-o").and_then(|t1| {Some(t1.to_owned())})
        .or(matches.get_one::<String>("config-file-i").and_then(|t1| {Some(t1.to_owned())}).and_then(|t| {
            Some(format!("{}/{}", repo_path, t)) })
            .or(if matches.get_flag("config-inside") {Some(format!("{}/gdep.yaml", repo_path))}
            else { Some("gdep.yaml".to_string()) })).unwrap();

    Config::load_from_file(&config_file_path)
}

fn get_repo(repo_path: &String, repo_url: Option<&String>) -> Result<Repository, GdepError> {
    match Repository::open(&repo_path) {
        Ok(repo) => Ok(repo),
        Err(_) => {
            if repo_url.is_none() {
                return Err(GdepError::LocalRepoNotFound(repo_path.to_owned()))
            }
            match Repository::clone(&repo_url.unwrap(), &repo_path) {
                Ok(repo) => {
                    Ok(repo)
                }
                Err(_) => {
                    Err(GdepError::RemoteRepoNotFound(repo_url.unwrap().to_owned()))
                }
            }
        }
    }
}

fn get_repo_config(config: &Config, provided_repo_path: &&String) -> Result<Repository, GdepError> {
    match &config.repo {
        RepoLike::Remote(r) => {get_repo(provided_repo_path, Some(&r))}
        RepoLike::Local(l) => {get_repo(&l, None)}
        RepoLike::Remote2(r, d) => {get_repo(&d, Some(&r))}
    }
}

fn run(matches: &ArgMatches) -> Result<(), GdepError> {
    let opt_repo_url = matches.get_one::<String>("repo-url");

    let binding = DEFAULT_REPO_PATH.to_string();
    let provided_repo_path = matches.get_one::<String>("repo-path").or(Some(&binding)).unwrap();

    let config_in_repo = matches.get_flag("config-inside") || matches.get_one::<String>("config-file-i").is_some();

    let (repo, repo_path, config) = if config_in_repo {
        let repo = get_repo(provided_repo_path, opt_repo_url)?;
        let repo_path = repo.path().parent().unwrap().to_str().unwrap().to_string();
        (repo, provided_repo_path.clone(), load_cfg(&matches, &repo_path)?)
    } else {
        let config = conv_err!(load_cfg(&matches, &provided_repo_path), Error::from_str("Could not load config 2"))?;
        let repo = get_repo_config(&config, &provided_repo_path)?;
        (repo, provided_repo_path.to_owned(), config)
    };

    let branch = matches.get_one::<String>("branch").and_then(|t| { Some(t.clone()) }).or(Some(get_default_branch(&repo)?)).unwrap();

    match execute(config, repo_path, branch) {
        None => {
            Ok(())
        }
        Some(err) => {
            Err(err)
        }
    }
}

fn main() {
    let matches = clap::Command::new(NAME)
        .about(DESCRIPTION)
        .version(VERSION)
        .color(ColorChoice::Never)
        .disable_version_flag(true)
        .arg(Arg::new("repo-url")
            .long("remote-repo")
            .short('r')
            .help("Remote repo to clone")
            .value_hint(clap::ValueHint::Url)
            .action(clap::ArgAction::Set))
        .arg(Arg::new("repo-path")
            .long("local-repo")
            .short('l')
            .help("Local repo to use. If paired with --remote-repo, this acts as a destination path. Ignored if it already exists")
            .value_hint(clap::ValueHint::DirPath)
            .action(clap::ArgAction::Set))
        .arg(Arg::new("config-file-i")
            .long("repo-config")
            .short('c')
            .help("Config file name (inside of repo). Defaults to <repo>/<config-file-i>")
            .value_hint(clap::ValueHint::FilePath)
            .action(clap::ArgAction::Set))
        .arg(Arg::new("config-file-o")
            .long("static-config")
            .short('s')
            .help("Config file name (outside of repo). Overwrites --repo-config. Defaults to <repo>/gdep.yaml (uses --repo-config)")
            .value_hint(clap::ValueHint::FilePath)
            .action(clap::ArgAction::Set))
        .arg(Arg::new("branch")
            .long("branch")
            .short('b')
            .help("Set the branch to use. Will otherwise be auto-inferred to main or master")
            .value_hint(clap::ValueHint::FilePath)
            .action(clap::ArgAction::Set))
        .arg(Arg::new("config-inside")
            .long("config-inside")
            .short('i')
            .help("Config file is inside the repo. Only used if neither --repo-config nor --static-config are provided")
            .action(clap::ArgAction::SetTrue))
        .arg(Arg::new("version")
            .short('v')
            .long("version")
            .help("Displays the version")
            .action(clap::ArgAction::Version))
        .arg(Arg::new("debug")
            .long("debug")
            .short('d')
            .help("Enable debug mode -> print errors as reals [currently unused]")
            .action(clap::ArgAction::SetTrue))
        .get_matches();
    
    let result = run(&matches);
    if result.is_err() {
        println!("Gdep Error: {}", result.unwrap_err())
    }
}