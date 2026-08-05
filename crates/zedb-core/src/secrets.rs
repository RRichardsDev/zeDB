//! Passwords live in the OS keychain (macOS Keychain via the `keyring`
//! crate), keyed by service "zedb" + connection name. The config file on
//! disk never contains secret material.

const SERVICE: &str = "zedb";

pub type SecretError = keyring::Error;

fn entry(connection_name: &str) -> Result<keyring::Entry, SecretError> {
    keyring::Entry::new(SERVICE, connection_name)
}

pub fn set_password(connection_name: &str, password: &str) -> Result<(), SecretError> {
    entry(connection_name)?.set_password(password)
}

/// `Ok(None)` when no password is stored for this connection.
pub fn get_password(connection_name: &str) -> Result<Option<String>, SecretError> {
    match entry(connection_name)?.get_password() {
        Ok(pw) => Ok(Some(pw)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e),
    }
}

pub fn delete_password(connection_name: &str) -> Result<(), SecretError> {
    match entry(connection_name)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e),
    }
}

/// Move a stored password when a connection is renamed.
pub fn rename(old_name: &str, new_name: &str) -> Result<(), SecretError> {
    if let Some(pw) = get_password(old_name)? {
        set_password(new_name, &pw)?;
        delete_password(old_name)?;
    }
    Ok(())
}
