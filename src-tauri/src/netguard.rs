//! Strict Offline Mode, enforced in Rust (hiding buttons is not enough).
//! Every function that can reach another machine calls `guard()` first.

use crate::agent::load_config;

#[cfg(feature = "network")]
pub fn strict() -> bool {
    load_config().offline()
}

/// Build without the `network` feature: offline is permanent and cannot be switched off.
#[cfg(not(feature = "network"))]
pub fn strict() -> bool {
    true
}

/// Err("ERR_OFFLINE") while strict offline mode is on.
pub fn guard() -> Result<(), String> {
    if strict() {
        Err("ERR_OFFLINE".to_string())
    } else {
        Ok(())
    }
}

#[tauri::command]
pub fn offline_get() -> bool {
    strict()
}

#[tauri::command]
pub fn offline_set(on: bool) -> Result<(), String> {
    if !on && !cfg!(feature = "network") {
        return Err("ERR_OFFLINE".to_string());
    }
    let mut c = load_config();
    c.offline = Some(on);
    crate::agent::save_config(&c)?;
    if on {
        // Stop the Telegram listener right away.
        crate::telegram::stop_listener();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_follows_strict_and_uses_a_translatable_code() {
        if strict() {
            assert_eq!(guard(), Err("ERR_OFFLINE".to_string()));
        } else {
            assert!(guard().is_ok());
        }
    }

    #[test]
    fn fresh_install_is_offline_by_default() {
        assert!(crate::agent::Config::default().offline());
    }
}
