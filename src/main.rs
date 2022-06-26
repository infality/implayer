#![windows_subsystem = "windows"]

use std::time::{Duration, Instant};
mod app;
mod clipboard;
mod output;
mod player;

use glutin::{
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    WindowedContext,
};
use imgui_winit_support::WinitPlatform;

const TITLE: &str = "Playlist Player";

type Window = WindowedContext<glutin::PossiblyCurrent>;

fn main() {
    let (event_loop, window) = create_window();
    let (mut winit_platform, mut imgui_context) = imgui_init(&window);
    imgui_context.style_mut().cell_padding = [0.0, 0.0];
    imgui_context.style_mut().window_padding = [0.0, 0.0];
    imgui_context.style_mut().window_border_size = 0.0;
    imgui_context.style_mut().scrollbar_rounding = 0.0;

    imgui_context.style_mut()[imgui::StyleColor::Text] = app::TEXT1;
    imgui_context.style_mut()[imgui::StyleColor::FrameBg] = app::DARK2;
    imgui_context.style_mut()[imgui::StyleColor::FrameBgHovered] = app::HOVERED_BG;
    imgui_context.style_mut()[imgui::StyleColor::FrameBgActive] = app::ACTIVE_BG;
    imgui_context.style_mut()[imgui::StyleColor::SliderGrab] = app::PRIMARY2;
    imgui_context.style_mut()[imgui::StyleColor::SliderGrabActive] = app::PRIMARY2;
    imgui_context.style_mut()[imgui::StyleColor::Button] = app::DARK2;
    imgui_context.style_mut()[imgui::StyleColor::ButtonHovered] = app::HOVERED_BG;
    imgui_context.style_mut()[imgui::StyleColor::ButtonActive] = app::ACTIVE_BG;
    imgui_context.style_mut()[imgui::StyleColor::Header] = app::PRIMARY1;
    imgui_context.style_mut()[imgui::StyleColor::HeaderHovered] = app::PRIMARY2;
    imgui_context.style_mut()[imgui::StyleColor::HeaderActive] = app::PRIMARY1;

    let gl = glow_context(&window);

    let mut ig_renderer = imgui_glow_renderer::AutoRenderer::initialize(gl, &mut imgui_context)
        .expect("failed to create renderer");

    let mut last_frame = Instant::now();

    if let Some(backend) = clipboard::init() {
        imgui_context.set_clipboard_backend(backend);
    } else {
        eprintln!("Failed to initialize clipboard");
    }

    #[cfg(not(target_os = "windows"))]
    let hwnd = None;

    #[cfg(target_os = "windows")]
    let hwnd = {
        use raw_window_handle::windows::WindowsHandle;
        use raw_window_handle::HasRawWindowHandle;

        let handle: WindowsHandle = match window.window().raw_window_handle() {
            raw_window_handle::RawWindowHandle::Windows(handle) => handle,
            _ => panic!(),
        };
        Some(handle.hwnd)
    };

    let mut state = app::initialize(hwnd);

    let mut redraws_required = 0;
    let mut fast_redrawing = false;

    event_loop.run(move |event, _, control_flow| {
        match event {
            Event::NewEvents(_) => {
                // This is executed even for events that occur in different windows:
                // https://github.com/rust-windowing/winit/issues/1634
            }
            Event::MainEventsCleared => {
                app::handle_media_keys(&mut state);
                if redraws_required > 0
                    || (fast_redrawing && Instant::now() - last_frame >= Duration::from_millis(200))
                    || Instant::now() - last_frame >= Duration::from_millis(1000)
                {
                    redraws_required -= 1;

                    winit_platform
                        .prepare_frame(imgui_context.io_mut(), window.window())
                        .expect("Failed to prepare frame");
                    window.window().request_redraw();
                }
                // If redraw is still requested, immediately loop again. Otherwise, wait for the
                // next event.
                if redraws_required > 0 {
                    *control_flow = ControlFlow::Poll;
                } else if fast_redrawing {
                    *control_flow = ControlFlow::WaitUntil(
                        Instant::now()
                            .checked_add(Duration::from_millis(200))
                            .unwrap(),
                    );
                } else {
                    *control_flow = ControlFlow::WaitUntil(
                        Instant::now()
                            .checked_add(Duration::from_millis(1000))
                            .unwrap(),
                    );
                }
            }
            Event::RedrawRequested(_) => {
                let now = Instant::now();
                imgui_context.io_mut().update_delta_time(now - last_frame);
                last_frame = now;

                let ui = imgui_context.frame();

                if false {
                    let mut a = true;
                    ui.show_demo_window(&mut a);
                } else {
                    let client_size = window.window().inner_size();
                    fast_redrawing = app::draw(
                        &ui,
                        client_size.width as f32,
                        client_size.height as f32,
                        &mut state,
                    );
                }

                winit_platform.prepare_render(&ui, window.window());
                let draw_data = ui.render();

                // This is the only extra render step to add
                ig_renderer
                    .render(draw_data)
                    .expect("error rendering imgui");

                window.swap_buffers().unwrap();
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => *control_flow = ControlFlow::Exit,
            event => {
                // We may need to redraw twice after an event. The first draw may make changes to
                // the GUI that are not reflected until the second draw. In some cases more redraws
                // may be needed.
                if let Event::WindowEvent { .. } = event {
                    redraws_required = 2;
                }
                winit_platform.handle_event(imgui_context.io_mut(), window.window(), &event);
            }
        }
    })
}

fn create_window() -> (EventLoop<()>, Window) {
    let event_loop = glutin::event_loop::EventLoop::new();
    let window = glutin::window::WindowBuilder::new()
        .with_title(TITLE)
        .with_inner_size(glutin::dpi::LogicalSize::new(1500, 780));
    let window = glutin::ContextBuilder::new()
        .with_vsync(true)
        .build_windowed(window, &event_loop)
        .expect("could not create window");
    let window = unsafe {
        window
            .make_current()
            .expect("could not make window context current")
    };
    (event_loop, window)
}

fn glow_context(window: &Window) -> glow::Context {
    unsafe { glow::Context::from_loader_function(|s| window.get_proc_address(s).cast()) }
}

fn imgui_init(window: &Window) -> (WinitPlatform, imgui::Context) {
    let mut imgui_context = imgui::Context::create();
    imgui_context.set_ini_filename(None);

    let mut winit_platform = WinitPlatform::init(&mut imgui_context);
    winit_platform.attach_window(
        imgui_context.io_mut(),
        window.window(),
        imgui_winit_support::HiDpiMode::Rounded,
    );

    imgui_context
        .fonts()
        .add_font(&[imgui::FontSource::TtfData {
            data: include_bytes!("DejaVuSans.ttf"),
            config: Some(imgui::FontConfig {
                glyph_ranges: imgui::FontGlyphRanges::from_slice(&[1, 65535, 0]),
                ..Default::default()
            }),
            size_pixels: 18.0,
        }]);
    imgui_context
        .fonts()
        .add_font(&[imgui::FontSource::TtfData {
            data: include_bytes!("NotoSansSymbols2-Regular.ttf"),
            config: Some(imgui::FontConfig {
                glyph_ranges: imgui::FontGlyphRanges::from_slice(&[1, 65535, 0]),
                ..Default::default()
            }),
            size_pixels: 32.0,
        }]);

    imgui_context.io_mut().font_global_scale = (1.0 / winit_platform.hidpi_factor()) as f32;

    (winit_platform, imgui_context)
}
