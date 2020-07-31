use std::{
    env, fs,
    io::Read as _,
    process::{self, Command, Stdio},
};

use anyhow::{anyhow, bail};
use xmas_elf::ElfFile;

use ggez;
use rand;

fn main() -> Result<(), anyhow::Error> {
    notmain().map(|opt_code| {
        if let Some(code) = opt_code {
            process::exit(code);
        }
    })
}

fn notmain() -> Result<Option<i32>, anyhow::Error> {
    let args = env::args().skip(1 /* program name */).collect::<Vec<_>>();

    if args.len() != 1 {
        bail!("expected exactly one argument. Syntax: `qemu-run <path-to-elf>`");
    }

    let path = &args[0];
    let bytes = fs::read(path)?;
    let elf = ElfFile::new(&bytes).map_err(anyhow::Error::msg)?;
    let table = elf2table::parse(&elf)?;

    let mut child = Command::new("qemu-system-arm")
        .args(&[
            "-cpu",
            "cortex-m3",
            "-machine",
            "lm3s6965evb",
            "-nographic",
            "-semihosting-config",
            "enable=on,target=native",
            "-kernel",
        ])
        .arg(path)
        .stdout(Stdio::piped())
        .spawn()?;

    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("failed to acquire child's stdout handle"))?;




    // Here we use a ContextBuilder to setup metadata about our game. First the title and author
    let (ctx, events_loop) = &mut ggez::ContextBuilder::new("snake", "Gray Olson")
        // Next we set up the window. This title will be displayed in the title bar of the window.
        .window_setup(ggez::conf::WindowSetup::default().title("Snake!"))
        // Now we get to set the size of the window, which we use our SCREEN_SIZE constant from earlier to help with
        .window_mode(ggez::conf::WindowMode::default().dimensions(SCREEN_SIZE.0, SCREEN_SIZE.1))
        // And finally we attempt to build the context and create the window. If it fails, we panic with the message
        // "Failed to build ggez context"
        .build()?;

    // Next we create a new instance of our GameState struct, which implements EventHandler
    let state = &mut UiState {
        frames: vec![],
        readbuf: [0; 256],
        exit_code: None,
        exited: false,
        stdout,
        table,
        child,
        colors: [Color::default(); 8],
        last: Instant::now(),
    };
    // And finally we actually run our game, passing in our context and state.
    ggez::event::run(ctx, events_loop, state).unwrap();

    Ok(Some(0)) // TODO
}

/// ----

use ggez::{
    event::EventHandler,
    GameResult,
    Context,
};

use std::time::{Instant, Duration};

struct UiState {
    frames: Vec<u8>,
    readbuf: [u8; 256],
    exit_code: Option<i32>,
    exited: bool,
    stdout: std::process::ChildStdout,
    table: decoder::Table,
    child: std::process::Child,
    colors: [Color; 8],
    last: Instant,
}

#[derive(Default, Copy, Clone)]
struct Color {
    red: u8,
    grn: u8,
    blu: u8,
}

impl EventHandler for UiState {
    fn update(&mut self, ctx: &mut Context) -> GameResult {
        if self.last.elapsed() >= Duration::from_millis(50) {
            self.last = Instant::now();
        } else {
            return Ok(());
        }

        let n = self.stdout.read(&mut self.readbuf)?;

        if n != 0 {
            self.frames.extend_from_slice(&self.readbuf[..n]);

            while let Ok((frame, consumed)) = decoder::decode(&self.frames, &self.table) {

                use decoder::Arg::{Format, Uxx};
                if !match frame.args.as_slice() {
                    [Format { ref format, ref args }] if format.contains("Pixel") && args.len() == 4 => {
                        if let &[Uxx(idx), Uxx(red), Uxx(grn), Uxx(blu)] = args.as_slice() {
                            println!("0x{:02X}{:02X}{:02X}@{}", red, grn, blu, idx);
                            self.colors[idx as usize] = Color { red: red as u8, grn: grn as u8, blu: blu as u8 };
                            true
                        } else {
                            false
                        }

                    },
                    _ => false,
                } {
                    println!("OTHER - {:?}", frame.args);
                }
                let n = self.frames.len();
                self.frames.rotate_left(consumed);
                self.frames.truncate(n - consumed);
            }
        }

        if n == 0 && self.exited {
            return Err(ggez::GameError::EventLoopError("brrt".into()));
        }

        if let Some(status) = self.child.try_wait()? {
            self.exit_code = status.code();
            // try to read once more before breaking out from this loop
            self.exited = true;
        }

        Ok(())
    }

    fn draw(&mut self, ctx: &mut Context) -> GameResult {
        println!("DRAWING");
        ggez::graphics::clear(ctx, [0.0, 1.0, 0.0, 1.0].into());
        for (i, pix) in self.colors.iter().enumerate() {
            let rectangle = ggez::graphics::Mesh::new_rectangle(
                ctx,
                ggez::graphics::DrawMode::fill(),
                GridPosition {
                    x: i as i16,
                    y: 0,
                }.into(),
                [pix.red as f32 / 255.0, pix.grn as f32 / 255.0, pix.blu as f32 / 255.0, 1.0].into()
            )?;
            ggez::graphics::draw(ctx, &rectangle, (ggez::mint::Point2 { x: 0.0, y: 0.0 },))?;
        }

        ggez::graphics::present(ctx)?;
        // We yield the current thread until the next update
        ggez::timer::yield_now();
        // And return success.
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct GridPosition {
    x: i16,
    y: i16,
}

/// We implement the `From` trait, which in this case allows us to convert easily between
/// a GridPosition and a ggez `graphics::Rect` which fills that grid cell.
/// Now we can just call `.into()` on a `GridPosition` where we want a
/// `Rect` that represents that grid cell.
impl From<GridPosition> for ggez::graphics::Rect {
    fn from(pos: GridPosition) -> Self {
        ggez::graphics::Rect::new_i32(
            pos.x as i32 * GRID_CELL_SIZE.0 as i32,
            pos.y as i32 * GRID_CELL_SIZE.1 as i32,
            GRID_CELL_SIZE.0 as i32,
            GRID_CELL_SIZE.1 as i32,
        )
    }
}

/// And here we implement `From` again to allow us to easily convert between
/// `(i16, i16)` and a `GridPosition`.
impl From<(i16, i16)> for GridPosition {
    fn from(pos: (i16, i16)) -> Self {
        GridPosition { x: pos.0, y: pos.1 }
    }
}

// The first thing we want to do is set up some constants that will help us out later.

// Here we define the size of our game board in terms of how many grid
// cells it will take up. We choose to make a 30 x 20 game board.
const GRID_SIZE: (i16, i16) = (8, 1);
// Now we define the pixel size of each tile, which we make 32x32 pixels.
const GRID_CELL_SIZE: (i16, i16) = (32, 32);

// Next we define how large we want our actual window to be by multiplying
// the components of our grid size by its corresponding pixel size.
const SCREEN_SIZE: (f32, f32) = (
    GRID_SIZE.0 as f32 * GRID_CELL_SIZE.0 as f32,
    GRID_SIZE.1 as f32 * GRID_CELL_SIZE.1 as f32,
);
