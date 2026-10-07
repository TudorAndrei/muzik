use fancy_regex::Regex;
use rusqlite::functions::{Context, FunctionFlags};
use rusqlite::types::{Value, ValueRef};
use rusqlite::{Connection, Error};

const FLAGS: FunctionFlags = FunctionFlags::SQLITE_UTF8.union(FunctionFlags::SQLITE_DETERMINISTIC);

/// Register the three scalar functions used by beets query SQL.
pub fn register_functions(connection: &Connection) -> Result<(), Error> {
    connection.busy_timeout(std::time::Duration::from_secs(30))?;
    connection.create_scalar_function("regexp", 2, FLAGS, |ctx| {
        let value = as_text(ctx, 0)?;
        let pattern = ctx.get::<String>(1)?;
        let regex = Regex::new(&pattern).map_err(user_error)?;
        regex.is_match(&value).map_err(user_error)
    })?;
    connection.create_scalar_function("unidecode", 1, FLAGS, |ctx| match ctx.get_raw(0) {
        ValueRef::Null => Ok(None),
        _ => Ok(Some(deunicode::deunicode(&as_text(ctx, 0)?))),
    })?;
    connection.create_scalar_function("bytelower", 1, FLAGS, |ctx| {
        Ok(match ctx.get_raw(0) {
            ValueRef::Blob(bytes) => Value::Blob(bytes.to_ascii_lowercase()),
            ValueRef::Text(bytes) => Value::Text(String::from_utf8_lossy(bytes).to_lowercase()),
            _ => Value::Null,
        })
    })?;
    Ok(())
}

fn as_text(ctx: &Context<'_>, index: usize) -> Result<String, Error> {
    Ok(match ctx.get_raw(index) {
        ValueRef::Null => "None".to_string(),
        ValueRef::Integer(value) => value.to_string(),
        ValueRef::Real(value) => value.to_string(),
        ValueRef::Text(value) | ValueRef::Blob(value) => {
            String::from_utf8(value.to_vec()).map_err(user_error)?
        }
    })
}

fn user_error(error: impl std::error::Error + Send + Sync + 'static) -> Error {
    Error::UserFunctionError(Box::new(error))
}
