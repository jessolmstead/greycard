use super::*;
use std::sync::mpsc;

fn os(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

fn launch(paths: &[&str]) -> Launch {
    Launch {
        paths: paths.iter().map(PathBuf::from).collect(),
        tokens: Vec::new(),
    }
}

/// A scratch folder for one test.
fn scratch(what: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("greycard-instance-{what}-{}", std::process::id()));
    greycard_render::testing::remove_dir_retry(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// An address of the test's own, and its folder to remove.
fn address(what: &str) -> (Address, Option<PathBuf>) {
    #[cfg(unix)]
    {
        let dir = scratch(what);
        (socket::at(dir.join("g.sock")), Some(dir))
    }
    #[cfg(windows)]
    {
        let suffix = format!("test-{what}-{}", std::process::id());
        (pipe::named(&suffix).unwrap(), None)
    }
}

fn done(dir: Option<PathBuf>) {
    if let Some(dir) = dir {
        greycard_render::testing::remove_dir_retry(&dir);
    }
}

#[test]
fn a_path_alone_or_nothing_takes_part_and_any_flag_does_not() {
    assert!(plain_launch(&[]));
    assert!(plain_launch(&os(&["/photos/a.CR3"])));
    assert!(plain_launch(&os(&["-vv", "/photos"])));
    assert!(plain_launch(&os(&["--verbose", "--", "-odd.CR3"])));
    // The start again after a lost GPU.
    assert!(plain_launch(&os(&["/photos/a.CR3", "--cpu-ops"])));
    // And after a model lost it.
    assert!(plain_launch(&os(&[
        "/photos/a.CR3",
        "--cpu-ops",
        "--after-model-loss"
    ])));
    assert!(!plain_launch(&os(&["a.CR3", "b.CR3"])));
    assert!(!plain_launch(&os(&["a.CR3", "--export", "out.jpg"])));
    assert!(!plain_launch(&os(&["--export", "out.jpg", "a.CR3"])));
    assert!(!plain_launch(&os(&["a.CR3", "--snapshot", "s.png"])));
    assert!(!plain_launch(&os(&["--grid"])));
    assert!(!plain_launch(&os(&["-x"])));
}

#[test]
fn a_launch_goes_over_the_wire_and_comes_back_as_it_was() {
    let mut paths = vec![
        PathBuf::from("/photos/2026/a.CR3"),
        PathBuf::from("/photos/a folder with spaces/b.NEF"),
        PathBuf::from("/photos/ünïcode/ç.RAF"),
    ];
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        // Latin-1 bytes in a name: no UTF-8.
        paths.push(PathBuf::from(OsString::from_vec(
            b"/photos/caf\xe9.ARW".to_vec(),
        )));
        assert!(paths[3].to_str().is_none());
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        // An unpaired surrogate: no UTF-8 either.
        let wide: Vec<u16> = r"C:\photos\a"
            .encode_utf16()
            .chain([0xd800, 0x41])
            .collect();
        paths.push(PathBuf::from(OsString::from_wide(&wide)));
    }
    // With the desktop's tokens.
    let sent = Launch {
        paths,
        tokens: vec![
            ("XDG_ACTIVATION_TOKEN".into(), "kwin-1234".into()),
            ("DESKTOP_STARTUP_ID".into(), "greycard-1_TIME0".into()),
        ],
    };
    let mut wire = Vec::new();
    write_launch(&mut wire, &sent).unwrap();
    assert_eq!(read_launch(&mut wire.as_slice()).unwrap(), sent);
    // Nothing sent is a launch with no path and no token.
    let mut wire = Vec::new();
    write_launch(&mut wire, &Launch::default()).unwrap();
    assert_eq!(
        read_launch(&mut wire.as_slice()).unwrap(),
        Launch::default()
    );
    // Another program's bytes, or a message cut short, are refused.
    assert!(read_launch(&mut &b"GET / HTTP/1.1\r\n"[..]).is_err());
    let mut wire = Vec::new();
    write_launch(&mut wire, &sent).unwrap();
    wire.truncate(wire.len() - 1);
    assert!(read_launch(&mut wire.as_slice()).is_err());
}

/// A path is handed over as given, made absolute and no more: a link
/// is not followed, so the frame lands where a cold start puts it.
#[test]
fn a_path_handed_over_is_absolute_and_not_canonical() {
    let dir = scratch("absolute");
    std::fs::write(dir.join("a.CR3"), b"x").unwrap();
    assert_eq!(absolute(None), Some(Vec::new()));
    assert_eq!(absolute(Some(&dir.join("gone.CR3"))), None);
    let here = std::env::current_dir().unwrap();
    let relative = std::fs::read_dir(&here)
        .unwrap()
        .flatten()
        .map(|e| PathBuf::from(e.file_name()))
        .next()
        .unwrap();
    assert_eq!(absolute(Some(&relative)), Some(vec![here.join(&relative)]));
    #[cfg(unix)]
    {
        let link = dir.join("link");
        std::os::unix::fs::symlink(&dir, &link).unwrap();
        let through = link.join("a.CR3");
        assert_eq!(absolute(Some(&through)), Some(vec![through.clone()]));
        // `..` is kept too, as the file system resolves it.
        let back = link.join("..").join(dir.file_name().unwrap()).join("a.CR3");
        assert_eq!(absolute(Some(&back)), Some(vec![back.clone()]));
    }
    greycard_render::testing::remove_dir_retry(&dir);
}

/// The runtime folder only when it is the user's alone; one others may
/// enter gives way to the cache folder.
#[cfg(unix)]
#[test]
fn the_socket_is_in_a_private_runtime_folder_else_the_cache_s() {
    use std::os::unix::fs::PermissionsExt;
    let dir = scratch("runtime");
    let runtime = dir.join("run");
    let cache = dir.join("cache");
    std::fs::create_dir_all(&runtime).unwrap();
    // SAFETY: getuid cannot fail.
    let uid = unsafe { libc::getuid() };
    let mode = |m| std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(m)).unwrap();
    mode(0o700);
    assert_eq!(
        socket_path(Some(runtime.clone()), Some(cache.clone()), uid, "k"),
        Some(runtime.join(format!("greycard-{uid}-k.sock")))
    );
    mode(0o755);
    let in_cache = Some(
        cache
            .join("greycard")
            .join(format!("greycard-{uid}-k.sock")),
    );
    assert_eq!(
        socket_path(Some(runtime.clone()), Some(cache.clone()), uid, "k"),
        in_cache
    );
    // Another user's, or not there, or relative: the cache's too.
    mode(0o700);
    assert_eq!(
        socket_path(Some(runtime.clone()), Some(cache.clone()), uid + 1, "k"),
        Some(
            cache
                .join("greycard")
                .join(format!("greycard-{}-k.sock", uid + 1))
        )
    );
    assert_eq!(
        socket_path(Some(dir.join("gone")), Some(cache.clone()), uid, "k"),
        in_cache
    );
    assert_eq!(
        socket_path(Some(PathBuf::from("run")), Some(cache.clone()), uid, "k"),
        in_cache
    );
    assert_eq!(socket_path(None, None, uid, "k"), None);
    greycard_render::testing::remove_dir_retry(&dir);
}

/// A run with a configuration folder of its own meets only editors of
/// that folder.
#[test]
fn two_configuration_folders_are_two_editors() {
    let (a, b) = (
        Path::new("/home/one/.config/greycard"),
        Path::new("/tmp/test/config/greycard"),
    );
    assert_ne!(config_key(Some(a)), config_key(Some(b)));
    assert_eq!(config_key(Some(a)), config_key(Some(a)));
    assert_eq!(config_key(Some(a)).len(), 8);
    let (one, other) = (
        Address::user(Some(a)).unwrap(),
        Address::user(Some(b)).unwrap(),
    );
    assert_ne!(one.to_string(), other.to_string());
    assert!(one.to_string().contains(&config_key(Some(a))));
}

#[cfg(unix)]
#[test]
fn a_socket_nobody_listens_on_is_removed_and_bound() {
    use std::os::unix::fs::PermissionsExt;
    let (address, dir) = address("stale");
    // An editor that crashed: the file there, nobody listening.
    drop(std::os::unix::net::UnixListener::bind(&address.socket).unwrap());
    assert!(address.socket.exists());
    let claimed = address.claim(&Launch::default());
    assert!(claimed.stale_removed);
    let Claim::Instance(instance) = claimed.claim else {
        panic!("not the instance");
    };
    // The user's alone.
    let mode = std::fs::metadata(&address.socket)
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
    // A launch now reaches it.
    let (tx, rx) = mpsc::channel();
    let mut served = instance.serve(move |l| {
        tx.send(l.paths).unwrap();
        true
    });
    assert!(matches!(
        address.claim(&Launch::default()).claim,
        Claim::Handed(_)
    ));
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(5)).unwrap(),
        Vec::<PathBuf>::new()
    );
    // Left, the socket goes; no file, and the next launch binds with
    // nothing stale to remove.
    served.leave();
    assert!(!address.socket.exists());
    let claimed = address.claim(&Launch::default());
    assert!(!claimed.stale_removed);
    assert!(matches!(claimed.claim, Claim::Instance(_)));
    // An editor that never got to serve (its start failed) takes its
    // socket away too.
    drop(claimed);
    assert!(!address.socket.exists());
    done(dir);
}

#[cfg(unix)]
#[test]
fn leaving_spares_a_socket_another_editor_put_there() {
    let (address, dir) = address("spare");
    let Claim::Instance(instance) = address.claim(&Launch::default()).claim else {
        panic!("not the instance");
    };
    let mut served = instance.serve(|_| true);
    // Removed under it and bound again by another.
    std::fs::remove_file(&address.socket).unwrap();
    let other = std::os::unix::net::UnixListener::bind(&address.socket).unwrap();
    served.leave();
    assert!(address.socket.exists());
    drop(other);
    done(dir);
}

#[test]
fn the_editor_takes_each_launch_in_turn_and_says_who_it_is() {
    let (address, dir) = address("two");
    let Claim::Instance(instance) = address.claim(&Launch::default()).claim else {
        panic!("not the instance");
    };
    let (tx, rx) = mpsc::channel();
    let served = instance.serve(move |l| {
        tx.send(l).unwrap();
        true
    });
    let first = launch(&["/photos/a.CR3"]);
    let second = Launch {
        tokens: vec![("XDG_ACTIVATION_TOKEN".into(), "t".into())],
        ..launch(&["/photos/2026"])
    };
    for sent in [&first, &second] {
        match address.claim(sent).claim {
            Claim::Handed(pid) => assert_eq!(pid, std::process::id()),
            _ => panic!("not handed"),
        }
    }
    let wait = Duration::from_secs(5);
    assert_eq!(rx.recv_timeout(wait).unwrap(), first);
    assert_eq!(rx.recv_timeout(wait).unwrap(), second);
    drop(served);
    done(dir);
}

/// No answer from an editor that is closing, or whose window could not
/// take the paths: the launch opens a window of its own rather than
/// lose its file.
#[test]
fn a_closing_editor_or_a_window_gone_answers_nothing() {
    let (address, dir) = address("left");
    let Claim::Instance(instance) = address.claim(&Launch::default()).claim else {
        panic!("not the instance");
    };
    let mut served = instance.serve(|l| l.paths.is_empty());
    // The window says no: no answer.
    assert!(matches!(
        address.claim(&launch(&["/photos/a.CR3"])).claim,
        Claim::Alone(_)
    ));
    // A launch already connected as the editor starts to close.
    #[cfg(unix)]
    let mut bare = socket::connect_bare(&address).unwrap();
    #[cfg(windows)]
    let mut bare = pipe::connect_bare(&address).unwrap();
    served.leave();
    let sent = hand(&mut bare, &Launch::default());
    assert!(sent.is_err(), "{sent:?}");
    done(dir);
}

#[test]
fn launches_at_once_make_one_editor() {
    let (address, dir) = address("race");
    let claims: Vec<(bool, Option<Served>)> = std::thread::scope(|s| {
        let racers: Vec<_> = (0..4)
            .map(|_| {
                s.spawn(|| match address.claim(&Launch::default()).claim {
                    // Served at once, as the window serves it.
                    Claim::Instance(instance) => (false, Some(instance.serve(|_| true))),
                    Claim::Handed(_) => (true, None),
                    Claim::Alone(e) => panic!("alone: {e}"),
                })
            })
            .collect();
        racers.into_iter().map(|r| r.join().unwrap()).collect()
    });
    let instances = claims.iter().filter(|(_, s)| s.is_some()).count();
    let handed = claims.iter().filter(|(h, _)| *h).count();
    assert_eq!((instances, handed), (1, 3));
    drop(claims);
    done(dir);
}
