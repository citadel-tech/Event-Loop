use std::{error::Error, result::Result as StdResult};

pub type Result<T> = StdResult<T, Box<dyn Error>>;
