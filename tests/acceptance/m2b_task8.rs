// M2b Task 8: pillar check for an idle daemon. Enabled from tests/acceptance_m2b.rs.
use super::*;

#[test]
fn a8_idle_daemon_footprint_under_30_mb() {
    let mut samples = vec![];
    for _ in 0..3 {
        let h = TempDir::new();
        ok(run(&h, &["list"]));
        let mut d = Daemon::start(&h);
        std::thread::sleep(Duration::from_secs(10));
        assert!(
            common::descendants(d.0.id()).is_empty(),
            "idle daemon launched a child"
        );
        samples.push(common::tree_footprint_kib(d.0.id()));
        assert_eq!(d.term(), Some(0));
    }
    samples.sort_unstable();
    println!("idle daemon footprint KiB: {samples:?}");
    assert!(samples[1] <= 30_720, "median {} KiB > 30 MB", samples[1]);
}
