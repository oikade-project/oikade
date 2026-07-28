// Copyright 2026 The Oikade Authors
// SPDX-License-Identifier: Apache-2.0

use super::parse_passcode;

#[test]
fn forbidden_passcodes_are_rejected() {
    for code in [0, 11_111_111, 12_345_678, 87_654_321, 99_999_999] {
        assert!(parse_passcode(&format!("{code:08}")).is_err());
    }
    assert_eq!(parse_passcode("20202021"), Ok(20_202_021));
    assert_eq!(parse_passcode("02022021"), Ok(2_022_021));
    assert!(parse_passcode("2022021").is_err());
}
