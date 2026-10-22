//! Intel game exec handoff. Steamworks initialization belongs to the game.
//! Also translates D3DMetal ahead of the Linux engine's games.
use obelisk_steam_native::native;

fn main() {
    let code = run();
    // SAFETY: terminate this helper after failed exec without running teardown
    // for libraries inherited through the game's discovery environment.
    unsafe { libc::_exit(code) }
}

fn run() -> i32 {
    let arguments: Vec<_> = std::env::args_os().collect();
    if arguments.len() == 2 && arguments[1] == "exec-game" {
        // SETEXEC receives the game its supervisor admits and the environment
        // the supervisor spawned this helper with.
        let _ = native::exec_game();
        return 1;
    }
    if arguments.len() == 4 && arguments[1] == "warm-d3dmetal" {
        return if native::warm_d3dmetal(&arguments[2], &arguments[3]) {
            0
        } else {
            1
        };
    }
    2
}
