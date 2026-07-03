//! Tolerant typed readers over a D-Bus property map (`a{sv}`) — shared by the
//! `BlueZ` and `UPower` folds. A missing or wrongly-typed property reads as `None`;
//! the folds decide what is required (§7: never invent a value).

use std::collections::HashMap;

use zbus::zvariant::OwnedValue;

/// The `a{sv}` property map shape every fold consumes.
pub type PropMap = HashMap<String, OwnedValue>;

/// A `b` property.
pub fn bool_prop(props: &PropMap, key: &str) -> Option<bool> {
    props.get(key).and_then(|v| v.downcast_ref::<bool>().ok())
}

/// An `s` property, owned.
pub fn str_prop(props: &PropMap, key: &str) -> Option<String> {
    props
        .get(key)
        .and_then(|v| v.downcast_ref::<&str>().ok())
        .map(str::to_owned)
}

/// A `y` (byte) property.
pub fn u8_prop(props: &PropMap, key: &str) -> Option<u8> {
    props.get(key).and_then(|v| v.downcast_ref::<u8>().ok())
}

/// An `n` (int16) property — e.g. `Device1.RSSI` (a signed dBm signal level).
pub fn i16_prop(props: &PropMap, key: &str) -> Option<i16> {
    props.get(key).and_then(|v| v.downcast_ref::<i16>().ok())
}

/// A `u` property.
pub fn u32_prop(props: &PropMap, key: &str) -> Option<u32> {
    props.get(key).and_then(|v| v.downcast_ref::<u32>().ok())
}

/// A `d` property.
pub fn f64_prop(props: &PropMap, key: &str) -> Option<f64> {
    props.get(key).and_then(|v| v.downcast_ref::<f64>().ok())
}

#[cfg(test)]
pub(crate) mod testutil {
    //! Property-map builders for the fold tests — the one place tests construct
    //! zvariant values.

    use zbus::zvariant::{OwnedValue, Str};

    use super::PropMap;

    /// A `PropMap` from (key, value) pairs.
    pub(crate) fn props(pairs: Vec<(&str, OwnedValue)>) -> PropMap {
        pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect()
    }

    /// An owned string value (`s`).
    pub(crate) fn s(v: &str) -> OwnedValue {
        OwnedValue::from(Str::from(v))
    }
}

#[cfg(test)]
mod tests {
    use zbus::zvariant::OwnedValue;

    use super::testutil::{props, s};
    use super::*;

    #[test]
    fn typed_readers_read_their_type_and_tolerate_absence_and_mismatch() {
        let map = props(vec![
            ("Powered", OwnedValue::from(true)),
            ("Alias", s("keyboard")),
            ("Percentage", OwnedValue::from(87.5_f64)),
            ("State", OwnedValue::from(2_u32)),
            ("Charge", OwnedValue::from(93_u8)),
            ("RSSI", OwnedValue::from(-58_i16)),
        ]);
        assert_eq!(bool_prop(&map, "Powered"), Some(true));
        assert_eq!(str_prop(&map, "Alias").as_deref(), Some("keyboard"));
        assert_eq!(f64_prop(&map, "Percentage"), Some(87.5));
        assert_eq!(u32_prop(&map, "State"), Some(2));
        assert_eq!(u8_prop(&map, "Charge"), Some(93));
        assert_eq!(i16_prop(&map, "RSSI"), Some(-58));

        // Absent key → None (never a default that lies).
        assert_eq!(bool_prop(&map, "Discovering"), None);
        assert_eq!(i16_prop(&map, "TxPower"), None);
        // Wrong type → None, not a panic (a hostile/buggy service can't crash
        // the seat).
        assert_eq!(bool_prop(&map, "Alias"), None);
        assert_eq!(str_prop(&map, "Powered"), None);
        assert_eq!(u8_prop(&map, "Percentage"), None);
        assert_eq!(i16_prop(&map, "Alias"), None);
    }
}
