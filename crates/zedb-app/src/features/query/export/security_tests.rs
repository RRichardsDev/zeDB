use super::partial_export_path;

#[test]
fn cancellation_uses_only_the_path_bound_to_the_active_download() {
    let active = std::path::Path::new("/tmp/active-export.csv");
    assert_eq!(partial_export_path(true, Some(active)), Some(active));
    assert_eq!(partial_export_path(false, Some(active)), None);
    assert_eq!(partial_export_path(true, None), None);
}
