//! Protected disk transaction shared by active and parked document saves.
use crate::{private_workspace::AtomicFileWriter, worker::OperationSummary};
use std::{fs, io, path::PathBuf};
use tiptoptyp::save_transaction::{
    ExpectedDiskState, SaveCompletion, SaveInput, WriteDurability, fingerprint,
};

pub(crate) struct SaveResult {
    pub path: PathBuf,
    pub completion: SaveCompletion,
    pub conflict: Option<ExpectedDiskState>,
}

impl OperationSummary for SaveResult {
    fn completion_summary(&self) -> String {
        match &self.completion.result {
            Ok(committed) => match &committed.durability {
                WriteDurability::Synchronized => format!("Saved {}", self.path.display()),
                WriteDurability::Uncertain(error) => format!(
                    "Saved {}, durability uncertain: {error}",
                    self.path.display()
                ),
            },
            Err(error) => format!("Could not save {}: {error}", self.path.display()),
        }
    }
}

pub(crate) fn execute(input: SaveInput) -> SaveResult {
    execute_checked(input, || Ok(()))
}

// The injected hook runs under the same lease, before revalidation. Tests use
// barriers here rather than wall-time thresholds to control concurrent writers.
fn execute_checked(input: SaveInput, before_check: impl FnOnce() -> io::Result<()>) -> SaveResult {
    let path = input.path().to_owned();
    let mut conflict = None;
    let completion = input.execute_with(|input| {
        AtomicFileWriter::write_checked(input.path(), input.bytes(), || {
            before_check()?;
            if input.expected_disk() != ExpectedDiskState::Unchecked {
                let observed = match fs::read(input.path()) {
                    Ok(bytes) => ExpectedDiskState::Fingerprint(fingerprint(&bytes)),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        ExpectedDiskState::Missing
                    }
                    Err(error) => return Err(error),
                };
                if observed != input.expected_disk() {
                    conflict = Some(observed);
                    return Err(io::Error::other("file changed on disk"));
                }
            }
            Ok(())
        })
        .map_err(|error| error.to_string())
    });
    SaveResult {
        path,
        completion,
        conflict,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worker::{ExclusiveJob, LatestJobPoll};
    use std::{
        path::Path,
        sync::mpsc,
        thread,
        time::{Duration, Instant},
    };
    use tiptoptyp::{mitex_document::Document, save_transaction::SaveIntent};
    use tiptoptyp_core::document::{DocumentKind, WindowSessionId};

    fn input(path: &Path, source: &str, expected: ExpectedDiskState) -> SaveInput {
        let document = Document::<usize>::new(WindowSessionId::new(1), source, DocumentKind::Text);
        SaveInput::new(
            document
                .prepare_save(path.into(), DocumentKind::Text)
                .unwrap(),
            expected,
            SaveIntent::Auto,
            None,
        )
    }
    fn wait(job: &mut ExclusiveJob<SaveResult>) -> SaveResult {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match job.poll() {
                LatestJobPoll::Ready(result) => return result,
                LatestJobPoll::Failed(error) => panic!("{error}"),
                _ => {
                    assert!(Instant::now() < deadline);
                    thread::yield_now();
                }
            }
        }
    }

    #[test]
    fn slow_writer_does_not_block_dispatch_and_remains_protected_after_close() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("saved.txt");
        fs::write(&path, b"old").unwrap();
        let request = input(
            &path,
            "new",
            ExpectedDiskState::Fingerprint(fingerprint(b"old")),
        );
        let context = eframe::egui::Context::default();
        let (entered, inside) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        let mut job = ExclusiveJob::default();
        let worker_path = path.clone();
        job.start_and_repaint("slow-save-test", &context, move || {
            Ok(execute_checked(request, || {
                assert!(crate::resource_lock::is_locked_for_test(&worker_path));
                entered.send(()).unwrap();
                blocked.recv().unwrap();
                Ok(())
            }))
        })
        .unwrap();
        inside.recv_timeout(Duration::from_secs(5)).unwrap();
        // The UI dispatch returned and can paint while the writer is blocked.
        let mut painted = false;
        context
            .run_ui(Default::default(), |ui| {
                ui.label("Responsive");
                painted = true;
            })
            .drop_without_applying_deltas();
        assert!(painted);
        assert!(matches!(job.poll(), LatestJobPoll::Pending));
        assert!(
            job.start_and_repaint("duplicate", &context, || panic!("must not start"))
                .is_err()
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "old");
        drop(job);
        assert!(crate::worker::has_active_operations());
        release.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(message) =
                crate::worker::take_matching_detached_completion(&context, "slow-save-test")
            {
                assert!(message.contains("Saved"));
                assert!(message.contains("after its window closed"));
                break;
            }
            assert!(Instant::now() < deadline);
            thread::yield_now();
        }
        assert_eq!(fs::read_to_string(path).unwrap(), "new");
    }

    #[test]
    fn same_path_jobs_revalidate_after_the_previous_writer_under_one_lease() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("saved.txt");
        fs::write(&path, b"old").unwrap();
        let expected = ExpectedDiskState::Fingerprint(fingerprint(b"old"));
        let first = input(&path, "first", expected);
        let second = input(&root.path().join("./saved.txt"), "second", expected);
        let (entered, inside) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        let writer = thread::spawn(move || {
            execute_checked(first, || {
                entered.send(()).unwrap();
                blocked.recv().unwrap();
                Ok(())
            })
        });
        inside.recv_timeout(Duration::from_secs(5)).unwrap();
        let later = thread::spawn(move || execute(second));
        release.send(()).unwrap();
        assert!(writer.join().unwrap().completion.result.is_ok());
        let rejected = later.join().unwrap();
        assert!(rejected.completion.result.is_err());
        assert_eq!(
            rejected.conflict,
            Some(ExpectedDiskState::Fingerprint(fingerprint(b"first")))
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "first");
        fs::remove_file(&path).unwrap();
        assert_eq!(
            execute(input(&path, "wrong", expected)).conflict,
            Some(ExpectedDiskState::Missing)
        );
        assert!(
            execute(input(&path, "created", ExpectedDiskState::Missing))
                .completion
                .result
                .is_ok()
        );
        assert!(
            execute(input(&path, "must not replace", ExpectedDiskState::Missing))
                .completion
                .result
                .is_err()
        );
    }

    #[test]
    #[ignore = "optimized foreground-dispatch probe; not a wall-time CI assertion"]
    fn save_dispatch_cost_probe() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("probe.txt");
        let source = "x".repeat(128 * 1024);
        let context = eframe::egui::Context::default();
        println!("mode,sample,dispatch_ns,total_ns");
        for background in [false, true] {
            for sample in 0..8 {
                fs::write(&path, b"old").unwrap();
                let request = input(
                    &path,
                    &source,
                    ExpectedDiskState::Fingerprint(fingerprint(b"old")),
                );
                let operation = move || {
                    execute_checked(request, || {
                        thread::sleep(Duration::from_millis(10));
                        Ok(())
                    })
                };
                let started = Instant::now();
                let dispatch;
                let result = if background {
                    let mut job = ExclusiveJob::default();
                    job.start_and_repaint("save-probe", &context, move || Ok(operation()))
                        .unwrap();
                    dispatch = started.elapsed();
                    wait(&mut job)
                } else {
                    let result = operation();
                    dispatch = started.elapsed();
                    result
                };
                let total = started.elapsed();
                assert!(result.completion.result.is_ok());
                assert_eq!(fs::read_to_string(&path).unwrap(), source);
                if sample >= 3 {
                    println!(
                        "{background},{},{},{}",
                        sample - 3,
                        dispatch.as_nanos(),
                        total.as_nanos()
                    );
                }
            }
        }
    }
}
