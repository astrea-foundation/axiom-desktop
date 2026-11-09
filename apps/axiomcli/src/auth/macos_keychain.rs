//! Inspect and update the existing login-Keychain item without decrypting it.
//!
//! The keyring backend's `set_password` first reads the old password. Updating
//! the item reference instead preserves its access controls and avoids another
//! decryption approval. Never delete/recreate an item to change its ownership.

use security_framework::{
    item::{ItemClass, ItemSearchOptions, Limit, Reference, SearchResult},
    os::macos::{
        keychain::{SecKeychain, SecPreferencesDomain},
        keychain_item::SecKeychainItem,
    },
};

const ITEM_NOT_FOUND: i32 = -25300; // errSecItemNotFound

pub(super) fn contains(service: &str, account: &str) -> Result<bool, String> {
    let keychain = user_keychain()?;
    find(&keychain, service, account).map(|item| item.is_some())
}

pub(super) fn save(service: &str, account: &str, password: &[u8]) -> Result<(), String> {
    save_in(&user_keychain()?, service, account, password)
}

fn user_keychain() -> Result<SecKeychain, String> {
    // Match apple-native-keyring-store's default User domain exactly; do not
    // search another keychain or migrate the shared CLI/Desktop credential.
    SecKeychain::default_for_domain(SecPreferencesDomain::User).map_err(|error| error.to_string())
}

fn find(
    keychain: &SecKeychain,
    service: &str,
    account: &str,
) -> Result<Option<SecKeychainItem>, String> {
    let result = ItemSearchOptions::new()
        .keychains(std::slice::from_ref(keychain))
        .class(ItemClass::generic_password())
        .service(service)
        .account(account)
        .load_refs(true)
        .load_data(false)
        .limit(Limit::Max(1))
        .search();
    match result {
        Ok(items) => match items.into_iter().next() {
            Some(SearchResult::Ref(Reference::KeychainItem(item))) => Ok(Some(item)),
            _ => Err("the system credential store returned an invalid item reference".into()),
        },
        Err(error) if error.code() == ITEM_NOT_FOUND => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn save_in(
    keychain: &SecKeychain,
    service: &str,
    account: &str,
    password: &[u8],
) -> Result<(), String> {
    match find(keychain, service, account)? {
        Some(mut item) => item.set_password(password),
        None => keychain.add_generic_password(service, account, password),
    }
    .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use security_framework::os::macos::{
        keychain::CreateOptions, passwords::find_generic_password,
    };
    use std::{path::PathBuf, process::Command};

    use super::*;

    struct TestKeychain(PathBuf);

    impl Drop for TestKeychain {
        fn drop(&mut self) {
            // SecKeychainCreate registers its new file in the search list.
            // Remove only this temporary fixture, including that registration.
            if self.0.exists() {
                let _ = Command::new("/usr/bin/security")
                    .arg("delete-keychain")
                    .arg(&self.0)
                    .output();
            }
        }
    }

    #[test]
    fn metadata_lookup_and_in_place_rotation_use_only_the_selected_keychain_item() {
        // An isolated, synthetic keychain; never access the user's login items
        // and never display a permission dialog while running this test.
        let _no_ui = SecKeychain::disable_user_interaction().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let fixture = TestKeychain(directory.path().join("auth-test.keychain"));
        let keychain = CreateOptions::new()
            .password("synthetic-keychain-password")
            .create(&fixture.0)
            .unwrap();
        let service = "stream.axiom.test";
        assert!(find(&keychain, service, "staging").unwrap().is_none());
        save_in(&keychain, service, "staging", b"old-staging-token").unwrap();
        save_in(&keychain, service, "production", b"production-token").unwrap();
        assert!(find(&keychain, service, "staging").unwrap().is_some());
        save_in(&keychain, service, "staging", b"rotated-staging-token").unwrap();
        for (account, expected) in [
            ("staging", b"rotated-staging-token".as_slice()),
            ("production", b"production-token".as_slice()),
        ] {
            let (password, _) =
                find_generic_password(Some(std::slice::from_ref(&keychain)), service, account)
                    .unwrap();
            assert_eq!(password.as_ref(), expected);
        }
    }
}
