use thiserror::Error;

#[derive(Debug, Error)]
pub enum PathError {
    #[error("path is empty or too long")]
    Length,
    #[error("path is absolute")]
    Absolute,
    #[error("path has an empty, '.' or '..' component")]
    Component,
    #[error("path contains a forbidden character")]
    Character,
}

#[derive(Debug, Error)]
pub enum PortError {
    #[error("network: {0}")]
    Network(String),
    #[error("process: {0}")]
    Process(String),
    #[error("io: {0}")]
    Io(String),
    #[error("cancelled")]
    Cancelled,
}

#[derive(Debug, Error)]
pub enum RegistryError {
    #[error("plugin {0} needs plugin API {1}, this build provides {2}")]
    ApiMismatch(String, u32, u32),
    #[error("duplicate {0} id: {1}")]
    Duplicate(&'static str, String),
}

#[derive(Debug, Error)]
pub enum SettingError {
    #[error("unknown setting {0}")]
    Unknown(String),
    #[error("wrong type for {0}")]
    Type(String),
    #[error("{0} is out of range")]
    Range(String),
    #[error("{0} is not an allowed choice")]
    Choice(String),
}

#[derive(Debug, Error)]
pub enum LaunchError {
    #[error("unknown placeholder {{{0}}}")]
    Placeholder(String),
    #[error("runner has no launch template")]
    NoTemplate,
    #[error("{0}")]
    Other(String),
}
