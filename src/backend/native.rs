use crate::backend::Backend;
use crate::errors::SerialError;
use crate::settings::Settings;

pub fn open(port: &str, _settings: &Settings) -> Result<Box<dyn Backend>, SerialError> {
    Err(SerialError::Value(format!("cannot open '{port}' yet")))
}
