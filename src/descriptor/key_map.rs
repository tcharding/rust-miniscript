// SPDX-License-Identifier: CC0-1.0

//! A map of public key to secret key.

use core::iter;

use bitcoin::psbt::{GetKey, GetKeyError, KeyRequest};
use bitcoin::secp256k1::{Secp256k1, Signing};

#[cfg(doc)]
use super::Descriptor;
use super::{DescriptorKeyParseError, DescriptorPublicKey, DescriptorSecretKey, SinglePubKey};
use crate::prelude::{btree_map, BTreeMap};

/// Alias type for a map of public key to secret key.
///
/// This map is returned whenever a descriptor that contains secrets is parsed using
/// [`Descriptor::parse_descriptor`], since the descriptor will always only contain
/// public keys. This map allows looking up the corresponding secret key given a
/// public key from the descriptor.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct KeyMap {
    map: BTreeMap<DescriptorPublicKey, DescriptorSecretKey>,
}

impl KeyMap {
    /// Creates a new empty `KeyMap`.
    #[inline]
    pub fn new() -> Self { Self { map: BTreeMap::new() } }

    /// Inserts secret key into key map returning the associated public key.
    #[inline]
    pub fn insert<C: Signing>(
        &mut self,
        secp: &Secp256k1<C>,
        sk: DescriptorSecretKey,
    ) -> Result<DescriptorPublicKey, DescriptorKeyParseError> {
        let pk = sk.to_public(secp)?;
        if !self.map.contains_key(&pk) {
            self.map.insert(pk.clone(), sk);
        }
        Ok(pk)
    }

    /// Gets the secret key associated with `pk` if `pk` is in the map.
    #[inline]
    pub fn get(&self, pk: &DescriptorPublicKey) -> Option<&DescriptorSecretKey> { self.map.get(pk) }

    /// Returns the number of items in this map.
    #[inline]
    pub fn len(&self) -> usize { self.map.len() }

    /// Returns true if the map is empty.
    #[inline]
    pub fn is_empty(&self) -> bool { self.map.is_empty() }
}

impl Default for KeyMap {
    fn default() -> Self { Self::new() }
}

impl IntoIterator for KeyMap {
    type Item = (DescriptorPublicKey, DescriptorSecretKey);
    type IntoIter = btree_map::IntoIter<DescriptorPublicKey, DescriptorSecretKey>;

    #[inline]
    fn into_iter(self) -> Self::IntoIter { self.map.into_iter() }
}

impl iter::Extend<(DescriptorPublicKey, DescriptorSecretKey)> for KeyMap {
    #[inline]
    fn extend<T>(&mut self, iter: T)
    where
        T: IntoIterator<Item = (DescriptorPublicKey, DescriptorSecretKey)>,
    {
        self.map.extend(iter)
    }
}

impl GetKey for KeyMap {
    type Error = GetKeyError;

    fn get_key<C: Signing>(
        &self,
        key_request: KeyRequest,
        secp: &Secp256k1<C>,
    ) -> Result<Option<bitcoin::PrivateKey>, Self::Error> {
        Ok(self.map.iter().find_map(|(k, v)| {
            match k {
                DescriptorPublicKey::Single(ref pk) => match key_request {
                    KeyRequest::Pubkey(ref request) => match pk.key {
                        SinglePubKey::FullKey(ref pk) => {
                            if pk == request {
                                match v {
                                    DescriptorSecretKey::Single(ref sk) => Some(sk.key),
                                    _ => unreachable!("Single maps to Single"),
                                }
                            } else {
                                None
                            }
                        }
                        SinglePubKey::XOnly(_) => None,
                    },
                    _ => None,
                },
                // Performance: Might be faster to check the origin and then if it matches return
                // the key directly instead of calling `get_key` on the xpriv.
                DescriptorPublicKey::XPub(ref xpub) => {
                    let pk = xpub.xkey.public_key;
                    match key_request {
                        KeyRequest::Pubkey(ref request) => {
                            if pk == request.inner {
                                match v {
                                    DescriptorSecretKey::XPrv(xpriv) => {
                                        let xkey = xpriv.xkey;
                                        if let Ok(child) =
                                            xkey.derive_priv(secp, &xpriv.derivation_path)
                                        {
                                            Some(bitcoin::PrivateKey::new(
                                                child.private_key,
                                                xkey.network,
                                            ))
                                        } else {
                                            None
                                        }
                                    }
                                    _ => unreachable!("XPrv maps to XPrv"),
                                }
                            } else {
                                None
                            }
                        }
                        KeyRequest::Bip32(..) => match v {
                            DescriptorSecretKey::XPrv(xpriv) => {
                                // This clone goes away in next release of rust-bitcoin.
                                if let Ok(Some(sk)) = xpriv.xkey.get_key(key_request.clone(), secp)
                                {
                                    Some(sk)
                                } else {
                                    None
                                }
                            }
                            _ => unreachable!("XPrv maps to XPrv"),
                        },
                        _ => unreachable!("rust-bitcoin v0.32"),
                    }
                }
                DescriptorPublicKey::MultiXPub(ref xpub) => {
                    let pk = xpub.xkey.public_key;
                    match key_request {
                        KeyRequest::Pubkey(ref request) => {
                            if pk == request.inner {
                                match v {
                                    DescriptorSecretKey::MultiXPrv(xpriv) => {
                                        Some(xpriv.xkey.to_priv())
                                    }
                                    _ => unreachable!("MultiXPrv maps to MultiXPrv"),
                                }
                            } else {
                                None
                            }
                        }
                        KeyRequest::Bip32(..) => match v {
                            DescriptorSecretKey::MultiXPrv(xpriv) => {
                                // These clones goes away in next release of rust-bitcoin.
                                if let Ok(Some(sk)) = xpriv.xkey.get_key(key_request.clone(), secp)
                                {
                                    Some(sk)
                                } else {
                                    None
                                }
                            }
                            _ => unreachable!("MultiXPrv maps to MultiXPrv"),
                        },
                        _ => unreachable!("rust-bitcoin v0.32"),
                    }
                }
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    // use bitcoin::NetworkKind;
    use bitcoin::bip32::{ChildNumber, IntoDerivationPath, Xpriv};

    use super::*;
    use crate::Descriptor;

    #[test]
    fn get_key_single_key() {
        let secp = Secp256k1::new();

        let descriptor_sk_s =
            "[90b6a706/44'/0'/0'/0/0]cMk8gWmj1KpjdYnAWwsEDekodMYhbyYBhG8gMtCCxucJ98JzcNij";

        let single = match descriptor_sk_s.parse::<DescriptorSecretKey>().unwrap() {
            DescriptorSecretKey::Single(single) => single,
            _ => panic!("unexpected DescriptorSecretKey variant"),
        };

        let want_sk = single.key;
        let descriptor_s = format!("wpkh({})", descriptor_sk_s);
        let (_, keymap) = Descriptor::parse_descriptor(&secp, &descriptor_s).unwrap();

        let pk = want_sk.public_key(&secp);
        let request = KeyRequest::Pubkey(pk);
        let got_sk = keymap
            .get_key(request, &secp)
            .expect("get_key call errored")
            .expect("failed to find the key");
        assert_eq!(got_sk, want_sk)
    }

    #[test]
    fn get_key_xpriv_single_key_xpriv() {
        let secp = Secp256k1::new();

        let s = "xprv9s21ZrQH143K3QTDL4LXw2F7HEK3wJUD2nW2nRk4stbPy6cq3jPPqjiChkVvvNKmPGJxWUtg6LnF5kejMRNNU3TGtRBeJgk33yuGBxrMPHi";

        let xpriv = s.parse::<Xpriv>().unwrap();
        let xpriv_fingerprint = xpriv.fingerprint(&secp);

        // Sanity check.
        {
            let descriptor_sk_s = format!("[{}]{}", xpriv_fingerprint, xpriv);
            let descriptor_sk = descriptor_sk_s.parse::<DescriptorSecretKey>().unwrap();
            let got = match descriptor_sk {
                DescriptorSecretKey::XPrv(x) => x.xkey,
                _ => panic!("unexpected DescriptorSecretKey variant"),
            };
            assert_eq!(got, xpriv);
        }

        let want_sk = xpriv.to_priv();
        let descriptor_s = format!("wpkh([{}]{})", xpriv_fingerprint, xpriv);
        let (_, keymap) = Descriptor::parse_descriptor(&secp, &descriptor_s).unwrap();

        let pk = want_sk.public_key(&secp);
        let request = KeyRequest::Pubkey(pk);
        let got_sk = keymap
            .get_key(request, &secp)
            .expect("get_key call errored")
            .expect("failed to find the key");
        assert_eq!(got_sk, want_sk)
    }

    #[test]
    fn get_key_xpriv_child_depth_one() {
        let secp = Secp256k1::new();

        let s = "xprv9s21ZrQH143K3QTDL4LXw2F7HEK3wJUD2nW2nRk4stbPy6cq3jPPqjiChkVvvNKmPGJxWUtg6LnF5kejMRNNU3TGtRBeJgk33yuGBxrMPHi";
        let master = s.parse::<Xpriv>().unwrap();
        let master_fingerprint = master.fingerprint(&secp);

        let child_number = ChildNumber::from_hardened_idx(44).unwrap();
        let child = master.derive_priv(&secp, &[child_number]).unwrap();

        // Sanity check.
        {
            let descriptor_sk_s = format!("[{}/44']{}", master_fingerprint, child);
            let descriptor_sk = descriptor_sk_s.parse::<DescriptorSecretKey>().unwrap();
            let got = match descriptor_sk {
                DescriptorSecretKey::XPrv(ref x) => x.xkey,
                _ => panic!("unexpected DescriptorSecretKey variant"),
            };
            assert_eq!(got, child);
        }

        let want_sk = child.to_priv();
        let descriptor_s = format!("wpkh({}/44')", s);
        let (_, keymap) = Descriptor::parse_descriptor(&secp, &descriptor_s).unwrap();

        let pk = want_sk.public_key(&secp);
        let request = KeyRequest::Pubkey(pk);
        let got_sk = keymap
            .get_key(request, &secp)
            .expect("get_key call errored")
            .expect("failed to find the key");
        assert_eq!(got_sk, want_sk)
    }

    #[test]
    fn get_key_xpriv_with_path() {
        let secp = Secp256k1::new();

        let s = "xprv9s21ZrQH143K3QTDL4LXw2F7HEK3wJUD2nW2nRk4stbPy6cq3jPPqjiChkVvvNKmPGJxWUtg6LnF5kejMRNNU3TGtRBeJgk33yuGBxrMPHi";
        let master = s.parse::<Xpriv>().unwrap();
        let master_fingerprint = master.fingerprint(&secp);

        let first_external_child = "44'/0'/0'/0/0";
        let derivation_path = first_external_child.into_derivation_path().unwrap();

        let child = master.derive_priv(&secp, &derivation_path).unwrap();

        // Sanity check.
        {
            let descriptor_sk_s =
                format!("[{}/{}]{}", master_fingerprint, first_external_child, child);
            let descriptor_sk = descriptor_sk_s.parse::<DescriptorSecretKey>().unwrap();
            let got = match descriptor_sk {
                DescriptorSecretKey::XPrv(ref x) => x.xkey,
                _ => panic!("unexpected DescriptorSecretKey variant"),
            };
            assert_eq!(got, child);
        }

        let want_sk = child.to_priv();
        let descriptor_s = format!("wpkh({}/44'/0'/0'/0/*)", s);
        let (_, keymap) = Descriptor::parse_descriptor(&secp, &descriptor_s).unwrap();

        let key_source = (master_fingerprint, derivation_path);
        let request = KeyRequest::Bip32(key_source);
        let got_sk = keymap
            .get_key(request, &secp)
            .expect("get_key call errored")
            .expect("failed to find the key");

        assert_eq!(got_sk, want_sk)
    }
}