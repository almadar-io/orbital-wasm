//! The `gpu/*` answerer for a browser host: one `GpuHost` per program instance, over orbital-canvas's handle table and
//! the page's WebGPU device. The host installs it in its `Server` (`installGpu`), which then holds every `gpu/` ask
//! and answers it here by ticket (`answerGpu`), so no argument or answer ever crosses to JavaScript.

use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct GpuHost {
    pub(crate) inner: std::rc::Rc<orbital_canvas::GpuHost>,
}

impl Default for GpuHost {
    fn default() -> Self {
        Self::new()
    }
}

#[wasm_bindgen]
impl GpuHost {
    #[wasm_bindgen(constructor)]
    pub fn new() -> GpuHost {
        GpuHost {
            inner: std::rc::Rc::new(orbital_canvas::GpuHost::new()),
        }
    }

    #[wasm_bindgen(js_name = closeAll)]
    pub fn close_all(&self) {
        self.inner.close_all();
    }
}
