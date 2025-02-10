mod config;

use std::{env};
use std::path::PathBuf;
use std::process::{Child, ExitStatus};
use git2::{Repository, Error, BranchType, RemoteCallbacks, Cred};
use std::string::ToString;
use std::thread;
use std::sync::{Arc, Mutex, mpsc};
use clap::{Arg, ArgMatches, ColorChoice};
use run_script::ScriptOptions;
use run_script::types::IoOptions;
use crate::config::{Config, ConfigError, RepoLike};

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

fn update_sync(repo_path: Arc<String>, branch_name: Arc<String>, stop_flag: Arc<Mutex<bool>>, sender: mpsc::Sender<(bool, bool)>) {
    let mut err = false;
    let repo_x = Repository::open(&*repo_path);
    
    if repo_x.is_ok() {
        let repo = repo_x.unwrap();
        while !*stop_flag.lock().unwrap() {
            sender.send((false, false)).expect("Failed to send alive signal to main thread");
            
            let res = repo_update_cycle(&repo, &branch_name);
            if res.is_err() {
                err = true;
                break
            }

            let urs = res.unwrap();
            match urs {
                UpdateRelationState::Up2Date => { continue }
                UpdateRelationState::Ahead(a) => {
                    err = true;
                    if err {
                        println!("Repo is {a} ahead of the remote repo. Can not update")
                    }
                    break
                }
                UpdateRelationState::Behind(_) => {
                    err = update_repo(&repo).is_err();
                    if err {
                        println!("Failed to update repo!")
                    } else {
                        println!("Successfully updated local repo")
                    }
                    break
                }
                UpdateRelationState::AheadBehind(a, b) => {
                    if err {
                        println!("Repo is {a} ahead of the remote repo and {b} behind. Can not update")
                    }
                }
            }
        }
    }

    println!("Function execution stopped. Sending signal to main thread...");
    sender.send((err, true)).expect("Failed to send stop signal to main thread");
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

    let tree_id = index.write_tree_to(repo)?;  // Ensure index is tied to the repo
    let tree = repo.find_tree(tree_id)?;
    let sig = repo.signature()?;
    repo.commit(Some("HEAD"), &sig, &sig, "Merged changes", &tree, &[&head, &fetch_head])?;

    // Ensure working directory is updated
    repo.checkout_head(Some(
        git2::build::CheckoutBuilder::new().force()
    ))?;

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
    fetch_updates(repo, "origin", branch)?;
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

    while !rx.recv().expect("Failed to receive singal from update thread").1 {
        let boring_result = child.try_wait();
        if boring_result.is_err() {
            *stop_flag.lock().unwrap() = true;
            // TODO : add handling
            boring_result.expect("Errorororororoor");
            break
        } else {
            result = boring_result.unwrap();
        }
        if result.is_some() {
            break;
        }
    }

    if result.is_some() {
        if !result.unwrap().success() {
            println!("Running script failed with exit code: {}", result.unwrap());
        }
    }

    *stop_flag.lock().unwrap() = true;

    child.kill().expect("Failed to kill the subprocess");
    update_handle.join().expect("Function thread panicked");

    if do_rerun {
        execute(config, repo_path, branch_name);
    }
}

fn update_repo(repo: &Repository) -> Result<(), Error> {
    merge_main_branch(repo)?;
    Ok(())
}

fn load_cfg(matches: &ArgMatches, repo_path: &String) -> Result<Config, ()> {
    let config_file_path = matches.get_one::<String>("config-file-o").and_then(|t1| {Some(t1.to_owned())})
        .or(matches.get_one::<String>("config-file-i").and_then(|t1| {Some(t1.to_owned())}).and_then(|t| {
            Some(format!("{}/{}", repo_path, t)) })
            .or(if matches.get_flag("config-inside") {Some(format!("{}/gdep.yaml", repo_path))}
            else { Some("gdep.yaml".to_string()) })).unwrap();

    match Config::load_from_file(&config_file_path) {
        Ok(config) => { Ok(config) }
        Err(err) => {
            match err {
                ConfigError::ConfigFileNotFound => {
                    println!("Can not read config file (at '{config_file_path}')")
                }
                ConfigError::ScriptFileNotFound => {
                    println!("Can not read script file")
                }
                ConfigError::ParsingFailed(e) => {
                    println!("Can not parse config file, due to {e}")
                }
                ConfigError::MissingContent(w) => {
                    println!("Missing required property in config: '{w}'")
                }
            }
            Err(())
        }
    }
}

fn get_repo(repo_path: &String, repo_url: Option<&String>) -> Result<Repository, Error> {
    match Repository::open(&repo_path) {
        Ok(repo) => Ok(repo),
        Err(e) => {
            if repo_url.is_none() {
                println!("Can not find repository (under '{repo_path}'), consider adding --remote-repo <repository>");
                return Err(e)
            }
            Ok(Repository::clone(&repo_url.unwrap(), &repo_path)?)
        }
    }
}

fn get_repo_config(config: &Config) -> Result<Repository, Error> {
    match &config.repo {
        RepoLike::Remote(r) => {get_repo(&DEFAULT_REPO_PATH.to_string(), Some(&r))}
        RepoLike::Local(l) => {get_repo(&l, None)}
        RepoLike::Remote2(r, d) => {get_repo(&d, Some(&r))}
    }
}

fn main() -> Result<(), Error> {
    // TODO: Do error handling
    
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
        .get_matches();

    let opt_repo_url = matches.get_one::<String>("repo-url");

    let binding = DEFAULT_REPO_PATH.to_string();
    let provided_repo_path = matches.get_one::<String>("repo-path").or(Some(&binding)).unwrap();

    let repo_infer_cfg = matches.get_flag("config-inside") || matches.get_one::<String>("config-file-i").is_some();

    let (repo, repo_path, config) = if repo_infer_cfg {
        let repo = get_repo(provided_repo_path, opt_repo_url)?;
        let repo_path = repo.path().parent().unwrap().to_str().unwrap().to_string();
        let config = conv_err!(load_cfg(&matches, &repo_path), Error::from_str("Could not load config 1"))?;
        (repo, provided_repo_path.clone(), config)
    } else {
        let repo_path = DEFAULT_REPO_PATH.to_string();
        let config = conv_err!(load_cfg(&matches, &repo_path), Error::from_str("Could not load config 2"))?;
        let repo = get_repo_config(&config)?;
        (repo, repo_path.clone(), config)
    };

    let branch = matches.get_one::<String>("branch").and_then(|t| { Some(t.clone()) }).or(Some(get_default_branch(&repo)?)).unwrap();

    execute(config, repo_path, branch);

    Ok(())
}