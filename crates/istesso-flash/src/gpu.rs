//! Drawing with the GPU.
//!
//! The pictures are those of [`crate::picture`], pixel for pixel. What differs
//! is how a frame reaches the window system. An application that paints on
//! the CPU hands over its pixels in shared memory, and a compositor that
//! renders with the GPU has to upload them: 20 MB a frame at 3440 by 1440.
//! One that draws with the GPU hands over a buffer that is on the GPU
//! already, as most applications do. Measurements of a relay are made with
//! the test application drawing this way on the host (`--draw gpu`).
//!
//! OpenGL ES 3.0 through EGL on Linux; OpenGL 4.1 on macOS, where this path
//! is only for trying the picture out.

use std::num::NonZeroU32;

use glow::HasContext;
use glutin::config::{ConfigTemplateBuilder, GlConfig};
use glutin::context::{
    ContextApi, ContextAttributesBuilder, NotCurrentGlContext, PossiblyCurrentContext, Version,
};
use glutin::display::{GetGlDisplay, GlDisplay};
use glutin::surface::{GlSurface, Surface, SurfaceAttributesBuilder, SwapInterval, WindowSurface};
use glutin_winit::{DisplayBuilder, GlWindow};
use istesso_measure::{Rgb, Shade};
use winit::event_loop::ActiveEventLoop;
use winit::raw_window_handle::HasWindowHandle;
use winit::window::{Window, WindowAttributes};

use crate::picture::{self, Picture, Sheet};

#[cfg(target_os = "macos")]
const SHADER_VERSION: &str = "#version 410 core\n";
#[cfg(not(target_os = "macos"))]
const SHADER_VERSION: &str =
    "#version 300 es\nprecision highp float;\nprecision highp int;\nprecision highp sampler2D;\n";

/// One triangle that covers the window.
const VERTEX_SHADER: &str = "
void main() {
    vec2 corner = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));
    gl_Position = vec4(corner * 2.0 - 1.0, 0.0, 1.0);
}
";

/// What `picture::paint_text` and a grey fill do, one pixel at a time.
const FRAGMENT_SHADER: &str = "
uniform sampler2D sheet;
uniform ivec4 window;  // width, height, rows of the sheet, rows the text has moved
uniform ivec4 square;  // left, top, side, rim
uniform vec3 shade;
uniform vec3 paper;
uniform int text;      // 1: scrolling text, 0: the whole window in the shade
out vec4 colour;

void main() {
    // Counted from the top left, as the CPU paints.
    ivec2 at = ivec2(int(gl_FragCoord.x), window.y - 1 - int(gl_FragCoord.y));
    ivec2 from_square = at - square.xy;
    bool on_square = from_square.x >= 0 && from_square.y >= 0
        && from_square.x < square.z && from_square.y < square.z;
    bool on_shade = from_square.x >= square.w && from_square.y >= square.w
        && from_square.x < square.z - square.w && from_square.y < square.z - square.w;
    if (text == 0 || on_shade) {
        colour = vec4(shade, 1.0);
    } else if (on_square) {
        colour = vec4(paper, 1.0);
    } else {
        // The sheet's pixels are 0x00RRGGBB, stored low byte first.
        colour = vec4(texelFetch(sheet, ivec2(at.x, (at.y + window.w) % window.z), 0).bgr, 1.0);
    }
}
";

/// A window's OpenGL surface, and what is needed to paint the pictures on it.
pub struct Gpu {
    gl: glow::Context,
    surface: Surface<WindowSurface>,
    context: PossiblyCurrentContext,
    program: glow::Program,
    texture: glow::Texture,
    size: (u32, u32),
}

fn colour(rgb: Rgb) -> [f32; 3] {
    [
        f32::from(rgb.r) / 255.0,
        f32::from(rgb.g) / 255.0,
        f32::from(rgb.b) / 255.0,
    ]
}

impl Gpu {
    /// Opens the window together with a surface the GPU can draw to.
    pub fn open(
        event_loop: &ActiveEventLoop,
        attributes: WindowAttributes,
    ) -> Result<(Window, Gpu), String> {
        let failed = |what: &str, error: &dyn std::fmt::Display| format!("cannot {what}: {error}");
        let (window, config) = DisplayBuilder::new()
            .with_window_attributes(Some(attributes))
            .build(event_loop, ConfigTemplateBuilder::new(), |configs| {
                // Nothing is drawn that multisampling would improve.
                configs
                    .min_by_key(|config| config.num_samples())
                    .expect("the display offers no OpenGL configuration")
            })
            .map_err(|e| failed("set OpenGL up on this display", &e))?;
        let window = window.ok_or("cannot open a window for OpenGL")?;
        let handle = window
            .window_handle()
            .map_err(|e| failed("reach the window", &e))?
            .as_raw();
        let display = config.display();
        #[cfg(target_os = "macos")]
        let api = ContextApi::OpenGl(Some(Version::new(4, 1)));
        #[cfg(not(target_os = "macos"))]
        let api = ContextApi::Gles(Some(Version::new(3, 0)));
        let wanted = ContextAttributesBuilder::new()
            .with_context_api(api)
            .build(Some(handle));
        // SAFETY: the handle is that of `window`, which lives as long as the context: the
        // caller keeps both in the same place and drops them together.
        let context = unsafe { display.create_context(&config, &wanted) }
            .map_err(|e| failed("create an OpenGL context", &e))?;
        let surface_attributes = window
            .build_surface_attributes(SurfaceAttributesBuilder::<WindowSurface>::new())
            .map_err(|e| failed("reach the window", &e))?;
        // SAFETY: as above, the surface belongs to `window`.
        let surface = unsafe { display.create_window_surface(&config, &surface_attributes) }
            .map_err(|e| failed("create a surface to draw on", &e))?;
        let context = context
            .make_current(&surface)
            .map_err(|e| failed("use the OpenGL context", &e))?;
        // A frame goes out when it is drawn, not on the display's next refresh. Where the
        // window system does not allow that, it stays as it is.
        let _ = surface.set_swap_interval(&context, SwapInterval::DontWait);

        // SAFETY: the context made current above is the one the functions are loaded for.
        let gl = unsafe {
            glow::Context::from_loader_function_cstr(|name| display.get_proc_address(name))
        };
        // SAFETY: plain OpenGL calls on the current context, with objects created right here.
        let (program, texture) = unsafe {
            let program = gl.create_program()?;
            for (kind, source) in [
                (glow::VERTEX_SHADER, VERTEX_SHADER),
                (glow::FRAGMENT_SHADER, FRAGMENT_SHADER),
            ] {
                let shader = gl.create_shader(kind)?;
                gl.shader_source(shader, &format!("{SHADER_VERSION}{source}"));
                gl.compile_shader(shader);
                if !gl.get_shader_compile_status(shader) {
                    return Err(format!(
                        "cannot compile a shader: {}",
                        gl.get_shader_info_log(shader)
                    ));
                }
                gl.attach_shader(program, shader);
            }
            gl.link_program(program);
            if !gl.get_program_link_status(program) {
                return Err(format!(
                    "cannot link the shaders: {}",
                    gl.get_program_info_log(program)
                ));
            }
            gl.use_program(Some(program));
            // A core profile draws nothing without a vertex array, even an empty one.
            let vertices = gl.create_vertex_array()?;
            gl.bind_vertex_array(Some(vertices));
            let texture = gl.create_texture()?;
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            for (parameter, value) in [
                (glow::TEXTURE_MIN_FILTER, glow::NEAREST),
                (glow::TEXTURE_MAG_FILTER, glow::NEAREST),
                (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
            ] {
                gl.tex_parameter_i32(glow::TEXTURE_2D, parameter, value as i32);
            }
            gl.uniform_1_i32(gl.get_uniform_location(program, "sheet").as_ref(), 0);
            (program, texture)
        };
        let gpu = Gpu {
            gl,
            surface,
            context,
            program,
            texture,
            size: (0, 0),
        };
        Ok((window, gpu))
    }

    /// Takes over a sheet of text that was painted for the window's present size.
    pub fn upload(&mut self, sheet: &Sheet) {
        let bytes: Vec<u8> = sheet
            .pixels()
            .iter()
            .flat_map(|pixel| pixel.to_le_bytes())
            .collect();
        // SAFETY: the slice holds width * height pixels of four bytes, as the call says.
        unsafe {
            self.gl.bind_texture(glow::TEXTURE_2D, Some(self.texture));
            self.gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                sheet.width() as i32,
                sheet.height() as i32,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(&bytes)),
            );
        }
    }

    /// Paints the picture as it is now and hands the frame to the window system.
    ///
    /// `sheet_height` and `offset` are those of the sheet last uploaded; they are not
    /// looked at for the grey picture.
    pub fn draw(
        &mut self,
        width: u32,
        height: u32,
        what: Picture,
        sheet_height: usize,
        offset: usize,
        shade: Shade,
    ) -> Result<(), String> {
        if self.size != (width, height) {
            if let (Some(across), Some(down)) = (NonZeroU32::new(width), NonZeroU32::new(height)) {
                self.surface.resize(&self.context, across, down);
            }
            self.size = (width, height);
        }
        let (left, top, side) = picture::square(width as usize, height as usize);
        let rim = picture::rim(side);
        let [red, green, blue] = colour(shade.rgb());
        let [paper_red, paper_green, paper_blue] = colour(picture::PAPER);
        let gl = &self.gl;
        let at = |name: &str| unsafe { gl.get_uniform_location(self.program, name) };
        // SAFETY: plain OpenGL calls on the current context with this object's own program.
        unsafe {
            gl.viewport(0, 0, width as i32, height as i32);
            gl.uniform_4_i32(
                at("window").as_ref(),
                width as i32,
                height as i32,
                sheet_height.max(1) as i32,
                offset as i32,
            );
            gl.uniform_4_i32(
                at("square").as_ref(),
                left as i32,
                top as i32,
                side as i32,
                rim as i32,
            );
            gl.uniform_3_f32(at("shade").as_ref(), red, green, blue);
            gl.uniform_3_f32(at("paper").as_ref(), paper_red, paper_green, paper_blue);
            gl.uniform_1_i32(at("text").as_ref(), i32::from(what == Picture::Text));
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
        }
        self.surface
            .swap_buffers(&self.context)
            .map_err(|e| format!("cannot show a frame: {e}"))
    }
}
