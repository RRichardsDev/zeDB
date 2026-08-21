use super::*;

fn cloud(service_id: &str) -> zedb_core::CloudProvenance {
    zedb_core::CloudProvenance {
        org_id: "org-1".into(),
        service_id: service_id.into(),
    }
}

#[test]
fn password_rotation_starts_only_from_explicit_confirmation() {
    let expected = cloud("service-1");
    assert!(password_provision_start_allowed(
        ProvisionStage::Confirm,
        Some(&expected)
    ));
    assert!(!password_provision_start_allowed(
        ProvisionStage::Idle,
        Some(&expected)
    ));
    assert!(!password_provision_start_allowed(
        ProvisionStage::Working,
        Some(&expected)
    ));
    assert!(!password_provision_start_allowed(
        ProvisionStage::Confirm,
        None
    ));
}

#[test]
fn password_rotation_completion_is_bound_to_the_initiating_form() {
    let expected = cloud("service-1");
    let other = cloud("service-2");
    assert!(password_provision_completion_matches(
        ProvisionStage::Working,
        Some(&expected),
        &expected,
        true,
    ));
    assert!(!password_provision_completion_matches(
        ProvisionStage::Idle,
        Some(&expected),
        &expected,
        true,
    ));
    assert!(!password_provision_completion_matches(
        ProvisionStage::Working,
        Some(&other),
        &expected,
        true,
    ));
    assert!(!password_provision_completion_matches(
        ProvisionStage::Working,
        Some(&expected),
        &expected,
        false,
    ));
}
