use std::fs;
use yaml_rust2::{YamlLoader, Yaml};

pub enum Installation {
    LoadFile(String, String), // Name, Content
    LoadStr(String)
}

pub enum RepoLike {
    Remote(String),
    Local(String),
    Remote2(String, String)
}

pub struct Config {
    name: String,
    re_run: bool,
    exit_on_script_error: bool,
    exit_on_gdep_error: bool,
    installation: Installation,
    repo: RepoLike
}

pub enum ConfigError {
    ConfigFileNotFound,
    ScriptFileNotFound,
    ParsingFailed(String),
    MissingContent(String)
}

fn ld_yaml_docs(path: String) -> Result<Vec<Yaml>, ConfigError> {
    let content = fs::read_to_string(path).or_else(|e1| {Err(ConfigError::ConfigFileNotFound)})?;
    YamlLoader::load_from_str(&*content).or_else(|e| {Err(ConfigError::ParsingFailed(e.to_string()))})
}

fn ld_script_file(path: String) -> Result<String, ConfigError> {
    Ok(fs::read_to_string(path).or_else(|e1| { Err(ConfigError::ScriptFileNotFound) })?)
}

impl Config {
    pub fn load_from_file(path: String) -> Result<Self, ConfigError> {
        let doc = &ld_yaml_docs(path)?[0];
        let name = &doc["name"].as_str();
        let re_run = doc["rerun"].as_bool().is_some_and(|t| {t});
        let inst_file = doc["use_file"].as_bool().is_some_and(|t| {t});
        let exit_on_gdep_error = doc["gdep_terminate"].as_bool().is_some_and(|t| {t});
        let exit_on_script_error = doc["script_terminate"].as_bool().is_some_and(|t| {t});
        let script = &doc[if inst_file {"file_path"} else {"script"}].as_str();
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
        
        let installation = if inst_file {Installation::LoadFile(script.clone(), ld_script_file(script)?)} else {Installation::LoadStr(script)};

        Ok(Self {
            name: name.unwrap().to_string(),
            re_run,
            exit_on_script_error,
            exit_on_gdep_error,
            installation,
            repo
        })
    }
}