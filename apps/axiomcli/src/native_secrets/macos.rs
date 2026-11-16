//! Safe Security.framework calls, restricted to the user's selected Keychain.
use core_foundation::data::CFData;
use security_framework::{
    item::{
        ItemAddOptions, ItemAddValue, ItemClass, ItemSearchOptions, ItemUpdateOptions,
        ItemUpdateValue, Location, SearchResult, update_item,
    },
    os::macos::{
        keychain::{SecKeychain, SecPreferencesDomain},
        passwords::find_generic_password,
    },
};

use super::{Backend, KEYRING_SERVICE, Result, invalid};
use zeroize::Zeroizing;

const ITEM_NOT_FOUND: i32 = -25300;

pub(super) struct Keychain {
    keychain: SecKeychain,
    account: String,
}

impl Keychain {
    pub(super) fn user(account: &str) -> Result<Self> {
        let keychain = SecKeychain::default_for_domain(SecPreferencesDomain::User)
            .map_err(|_| "The system credential store is unavailable")?;
        Ok(Self {
            keychain,
            account: account.to_owned(),
        })
    }

    fn query(&self, service: &str, account: &str) -> ItemSearchOptions {
        let mut query = ItemSearchOptions::new();
        query
            .keychains(std::slice::from_ref(&self.keychain))
            .class(ItemClass::generic_password())
            .service(service)
            .account(account);
        query
    }
}

impl Backend for Keychain {
    fn revision(&self) -> Result<Option<String>> {
        // kSecAttrComment is returned as the public "icmt" attribute key.
        // Return attributes only; never retrieve the password for this check.
        let result = self
            .query(KEYRING_SERVICE, &self.account)
            .load_attributes(true)
            .load_data(false)
            .search();
        match result {
            Ok(items) => {
                let attributes = items
                    .first()
                    .and_then(SearchResult::simplify_dict)
                    .ok_or_else(invalid)?;
                let revision = attributes.get("icmt").ok_or_else(invalid)?;
                if uuid::Uuid::parse_str(revision).is_err() {
                    return Err(invalid());
                }
                Ok(Some(revision.clone()))
            }
            Err(error) if error.code() == ITEM_NOT_FOUND => Ok(None),
            Err(_) => Err("Could not inspect the native secrets Keychain item".into()),
        }
    }

    fn read(&self) -> Result<Zeroizing<Vec<u8>>> {
        self.legacy_read(KEYRING_SERVICE, &self.account)?
            .ok_or_else(invalid)
    }

    fn write(&self, revision: &str, bytes: &[u8], exists: bool) -> Result<()> {
        if exists {
            // Preserve the item's ACL and change its revision in the same
            // operation as its data. Do not read the old password to update it.
            let mut update = ItemUpdateOptions::new();
            update
                .set_value(ItemUpdateValue::Data(CFData::from_buffer(bytes)))
                .set_comment(revision);
            update_item(&self.query(KEYRING_SERVICE, &self.account), &update)
        } else {
            let mut add = ItemAddOptions::new(ItemAddValue::Data {
                class: ItemClass::generic_password(),
                data: CFData::from_buffer(bytes),
            });
            add.set_service(KEYRING_SERVICE)
                .set_account_name(&self.account)
                .set_label("Axiom unlock")
                .set_comment(revision)
                .set_location(Location::FileKeychain(self.keychain.clone()));
            add.add()
        }
        .map_err(|_| {
            "Could not save native secrets in Keychain; existing keys were preserved".into()
        })
    }

    fn legacy_contains(&self, service: &str, account: &str) -> Result<bool> {
        match self
            .query(service, account)
            .load_refs(true)
            .load_data(false)
            .search()
        {
            Ok(items) => Ok(!items.is_empty()),
            Err(error) if error.code() == ITEM_NOT_FOUND => Ok(false),
            Err(_) => Err("Could not inspect the system credential store".into()),
        }
    }

    fn legacy_read(&self, service: &str, account: &str) -> Result<Option<Zeroizing<Vec<u8>>>> {
        match find_generic_password(Some(std::slice::from_ref(&self.keychain)), service, account) {
            Ok((password, _)) => Ok(Some(Zeroizing::new(password.as_ref().to_vec()))),
            Err(error) if error.code() == ITEM_NOT_FOUND => Ok(None),
            Err(_) => {
                Err("Native secrets are locked or unavailable; existing data was preserved".into())
            }
        }
    }

    fn legacy_delete(&self, service: &str, account: &str) -> Result<()> {
        match self.query(service, account).delete() {
            Ok(()) => Ok(()),
            Err(error) if error.code() == ITEM_NOT_FOUND => Ok(()),
            Err(_) => Err("Could not clear the old sign-in credential from Keychain".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_secrets::Store;
    use security_framework::os::macos::keychain::CreateOptions;
    use std::{path::PathBuf, process::Command};

    struct TestKeychain(PathBuf);
    impl Drop for TestKeychain {
        fn drop(&mut self) {
            if self.0.exists() {
                let _ = Command::new("/usr/bin/security")
                    .arg("delete-keychain")
                    .arg(&self.0)
                    .output();
            }
        }
    }

    #[test]
    fn selected_keychain_migrates_and_updates_data_and_revision_without_changing_other_items() {
        let _no_ui = SecKeychain::disable_user_interaction().unwrap();
        let root = tempfile::tempdir().unwrap();
        let fixture = TestKeychain(root.path().join("native-secrets-test.keychain"));
        let keychain = CreateOptions::new()
            .password("synthetic-keychain-password")
            .create(&fixture.0)
            .unwrap();
        let store = Store::new(&root.path().join("data")).unwrap();
        let backend = Keychain {
            keychain,
            account: store.scope.clone(),
        };
        backend
            .keychain
            .add_generic_password(
                crate::auth::KEYRING_SERVICE,
                "production",
                b"original-synthetic-token",
            )
            .unwrap();
        backend
            .keychain
            .add_generic_password(
                crate::local_storage::KEYRING_SERVICE,
                "account:desktop",
                &[0x73; 32],
            )
            .unwrap();
        backend
            .keychain
            .add_generic_password("unrelated-service", "production", b"keep-other-application")
            .unwrap();
        assert!(backend.revision().unwrap().is_none());
        store
            .read(&backend, crate::auth::KEYRING_SERVICE, "production")
            .unwrap();
        let initial = backend.revision().unwrap().unwrap();
        store
            .read(
                &backend,
                crate::local_storage::KEYRING_SERVICE,
                "account:desktop",
            )
            .unwrap();
        store
            .write(
                &backend,
                crate::auth::KEYRING_SERVICE,
                "production",
                b"rotated-synthetic-token",
            )
            .unwrap();
        assert_ne!(backend.revision().unwrap().unwrap(), initial);
        let reopened = Store::new(&root.path().join("data")).unwrap();
        assert_eq!(
            reopened
                .read(&backend, crate::auth::KEYRING_SERVICE, "production")
                .unwrap()
                .unwrap()
                .as_slice(),
            b"rotated-synthetic-token"
        );
        assert_eq!(
            reopened
                .read(
                    &backend,
                    crate::local_storage::KEYRING_SERVICE,
                    "account:desktop"
                )
                .unwrap()
                .unwrap()
                .as_slice(),
            &[0x73; 32]
        );
        reopened
            .delete(&backend, crate::auth::KEYRING_SERVICE, "production")
            .unwrap();
        assert!(
            !backend
                .legacy_contains(crate::auth::KEYRING_SERVICE, "production")
                .unwrap()
        );
        assert!(
            backend
                .legacy_contains(crate::local_storage::KEYRING_SERVICE, "account:desktop")
                .unwrap()
        );
        assert_eq!(
            backend
                .legacy_read("unrelated-service", "production")
                .unwrap()
                .unwrap()
                .as_slice(),
            b"keep-other-application"
        );
    }
}
