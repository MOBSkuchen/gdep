use std::{fmt, fs};
use std::path::{Path, PathBuf};
use yaml_rust2::{YamlLoader, Yaml};
use crate::{conv_err, conv_err_e};

pub enum RepoLike {
    Remote(String),
    Local(String),
    Remote2(String, String)
}

pub struct Config {
    pub name: String,
    pub re_run: bool,
    pub restart_after_update: bool,
    pub exit_on_script_error: bool,
    pub exit_on_gdep_error: bool,
    pub script: String,
    pub repo: RepoLike,
    pub cleanup: Option<String>
}

#[derive(Debug, Clone)]
pub enum ConfigError {
    ConfigFileNotFound,
    ScriptFileNotFound,
    ParsingFailed(String),
    MissingContent(String)
}
impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::ConfigFileNotFound => {
                write!(f, "Config file not found")
            },
            ConfigError::ScriptFileNotFound => {
                write!(f, "Script file not found")
            },
            ConfigError::ParsingFailed(err) => {
                write!(f, "Parsing failed: {}", err)
            },
            ConfigError::MissingContent(c) => {
                write!(f, "Missing mandatory property: {}", c)
            }
        }
    }
}

fn resolve_other_path(original: &Path, other: &Path) -> PathBuf {
    if other.is_absolute() {
        return other.to_path_buf();
    }

    let mut resolved_path = original.parent().unwrap_or_else(|| Path::new("")).to_path_buf();
    resolved_path.push(other);
    resolved_path
}

fn ld_yaml_docs(path: &String) -> Result<Vec<Yaml>, ConfigError> {
    let content = conv_err!(fs::read_to_string(path), ConfigError::ConfigFileNotFound)?;
    conv_err_e!(YamlLoader::load_from_str(&*content), ConfigError::ParsingFailed)
}

fn ld_script_file(cfg_path: &String, script_path: &String) -> Result<String, ConfigError> {
    let path = resolve_other_path(Path::new(cfg_path.as_str()), Path::new(script_path.as_str()));
    Ok(conv_err!(fs::read_to_string(path), ConfigError::ScriptFileNotFound)?)
}

impl Config {
    pub fn load_from_file(path: &String) -> Result<Self, ConfigError> {
        let doc = &ld_yaml_docs(path)?[0];
        let name = &doc["name"].as_str();
        let run_is_final = doc["final"].as_bool().is_some_and(|t| {t});
        let inst_file1 = doc["script_use_file"].as_bool().is_some_and(|t| {t});
        let inst_file2 = doc["script_use_file"].as_bool().is_some_and(|t| {t});
        let restart_after_update = doc["restart_update"].as_bool().is_some_and(|t| {t});
        let exit_on_gdep_error = !doc["gdep_err_ignore"].as_bool().is_some_and(|t| {t});
        let exit_on_script_error = !doc["script_err_ignore"].as_bool().is_some_and(|t| {t});
        let script = &doc[if inst_file1 {"file_path"} else {"script"}].as_str();
        let cleanup = &doc[if inst_file2 {"cleanup_file_path"} else {"cleanup"}].as_str();
        let local_repo = doc["local_repo"].as_bool().is_some_and(|t| {t});
        let repo = &doc["repo"].as_str();
        let into_path = &doc["into_path"].as_str();
        
        if name.is_none() {
            return Err(ConfigError::MissingContent("name".to_string()))
        }

        if script.is_none() {
            return Err(ConfigError::MissingContent("script".to_string()))
        }
        
        if repo.is_none() {
            return Err(ConfigError::MissingContent("repo".to_string()))
        }
        
        let repo = if local_repo {RepoLike::Local(repo.unwrap().to_string())} 
                            else {
                                if into_path.is_none() {
                                    RepoLike::Remote(repo.unwrap().to_string())
                                } else {
                                    RepoLike::Remote2(repo.unwrap().to_string(), into_path.unwrap().to_string())
                                }
                            };
        
        let script = script.unwrap().to_string();

        let installation = if inst_file1 {ld_script_file(&path, &script)?} else {script};
        let cleanup = if cleanup.is_some() {
            Some(if inst_file2 {ld_script_file(&path, &cleanup.unwrap().to_string())?} else {cleanup.unwrap().to_string()})
        } else {None};

        Ok(Self {
            name: name.unwrap().to_string(),
            re_run: !run_is_final,
            restart_after_update,
            exit_on_script_error,
            exit_on_gdep_error,
            script: installation,
            cleanup,
            repo
        })
    }
}