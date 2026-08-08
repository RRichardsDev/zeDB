//! Passwords live in the macOS Keychain and require user presence when
//! unlocked. Existing credentials from the original `keyring` backend are
//! migrated on first read.

use security_framework::passwords::{
    delete_generic_password_options, generic_password, set_generic_password_options,
    AccessControlOptions, PasswordOptions,
};

const SERVICE: &str = "dev.zedb.app.protected-credentials";
const LEGACY_SERVICE: &str = "zedb";
const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;
const ERR_SEC_MISSING_ENTITLEMENT: i32 = -34018;

#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    #[error("macOS Keychain error: {0}")]
    Keychain(#[from] security_framework::base::Error),
    #[error("saved credential is not valid UTF-8")]
    InvalidUtf8(#[from] std::string::FromUtf8Error),
}

fn options(connection_name: &str, protected: bool) -> PasswordOptions {
    let service = if protected { SERVICE } else { LEGACY_SERVICE };
    let mut options = PasswordOptions::new_generic_password(service, connection_name);
    if protected {
        options.use_protected_keychain();
    }
    options
}

fn protected_options(connection_name: &str) -> PasswordOptions {
    options(connection_name, true)
}

fn protected_write_options(connection_name: &str) -> PasswordOptions {
    let mut options = protected_options(connection_name);
    options.set_access_control_options(AccessControlOptions::USER_PRESENCE);
    options
}

fn read(connection_name: &str, protected: bool) -> Result<Option<String>, SecretError> {
    match generic_password(options(connection_name, protected)) {
        Ok(password) => Ok(Some(String::from_utf8(password)?)),
        Err(error) if error.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn delete(connection_name: &str, protected: bool) -> Result<(), SecretError> {
    match delete_generic_password_options(options(connection_name, protected)) {
        Ok(()) => Ok(()),
        Err(error) if error.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub fn set_password(connection_name: &str, password: &str) -> Result<(), SecretError> {
    set_generic_password_options(
        password.as_bytes(),
        protected_write_options(connection_name),
    )?;
    Ok(())
}

/// Returns `Ok(None)` when no password is stored for this connection.
///
/// A legacy credential is recreated with user-presence protection after its
/// first successful read. This can show the old authorization dialog once.
pub fn get_password(connection_name: &str) -> Result<Option<String>, SecretError> {
    if let Some(password) = read(connection_name, true)? {
        return Ok(Some(password));
    }

    let Some(password) = read(connection_name, false)? else {
        return Ok(None);
    };
    match set_password(connection_name, &password) {
        Ok(()) => delete(connection_name, false)?,
        Err(SecretError::Keychain(error)) if error.code() == ERR_SEC_MISSING_ENTITLEMENT => {
            // Local certificate signing without a provisioning profile cannot
            // create an access-controlled item. Keep the legacy item intact and
            // use its value for this session; a provisioned build will migrate
            // it on a later successful read.
        }
        Err(error) => return Err(error),
    }
    Ok(Some(password))
}

/// Store a secret as a standard Keychain item without user-presence
/// protection: for tokens read silently at every launch (e.g. the GitHub
/// OAuth token), where a biometric prompt would be wrong and where the
/// protected keychain is unavailable to unprovisioned dev builds.
pub fn set_plain(name: &str, value: &str) -> Result<(), SecretError> {
    set_generic_password_options(value.as_bytes(), options(name, false))?;
    Ok(())
}

pub fn get_plain(name: &str) -> Result<Option<String>, SecretError> {
    read(name, false)
}

pub fn delete_plain(name: &str) -> Result<(), SecretError> {
    delete(name, false)
}

pub fn delete_password(connection_name: &str) -> Result<(), SecretError> {
    delete(connection_name, true)?;
    delete(connection_name, false)
}

/// Move a stored password when a connection is renamed.
pub fn rename(old_name: &str, new_name: &str) -> Result<(), SecretError> {
    if let Some(password) = get_password(old_name)? {
        set_password(new_name, &password)?;
        delete_password(old_name)?;
    }
    Ok(())
}
