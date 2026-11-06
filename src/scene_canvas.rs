//! A browser scene canvas over orbital-canvas: any canvas-style render node (learning instruments, diagrams, a 2D
//! game `canvas`) mapped and drawn into the page's `<canvas>`, with orbital-canvas's own layout, clock and input.

use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct SceneCanvas {
    inner: orbital_canvas::web2d::WebSceneCanvas,
}

#[wasm_bindgen]
impl SceneCanvas {
    pub async fn create(canvas: web_sys::HtmlCanvasElement) -> Result<SceneCanvas, JsError> {
        let inner = orbital_canvas::web2d::WebSceneCanvas::new(canvas)
            .await
            .map_err(|e| JsError::new(&e))?;
        Ok(SceneCanvas { inner })
    }

    /// `node_json`: the render node (`{ "type": "physics-canvas", … }`).
    #[wasm_bindgen(js_name = setScene)]
    pub fn set_scene(&mut self, node_json: &str) -> Result<(), JsError> {
        let node: serde_json::Value = serde_json::from_str(node_json)
            .map_err(|e| JsError::new(&format!("node_json: {e}")))?;
        self.inner.set_scene(node).map_err(|e| JsError::new(&e))
    }

    pub fn resize(
        &mut self,
        css_width: f32,
        css_height: f32,
        pixel_ratio: f32,
    ) -> Result<(), JsError> {
        self.inner
            .resize(css_width, css_height, pixel_ratio)
            .map_err(|e| JsError::new(&e))
    }

    #[wasm_bindgen(js_name = setMotion)]
    pub fn set_motion(&mut self, enabled: bool) {
        self.inner.set_motion(enabled);
    }

    /// Advances the scene clock to `now_ms`; true while the page should keep drawing.
    pub fn frame(&mut self, now_ms: f64) -> bool {
        self.inner.frame(now_ms)
    }

    pub fn render(&mut self) -> Result<(), JsError> {
        self.inner.render().map_err(|e| JsError::new(&e))
    }

    /// A pointer event in CSS pixels: `touch-down`, `touch-move`, `touch-up` or `touch-cancel`.
    pub fn input(&mut self, kind: &str, x: f32, y: f32, button: i32) {
        self.inner.input(kind, x, y, button);
    }

    /// Whether the scene took the zoom; the page scrolls when it did not.
    pub fn zoom(&mut self, factor: f32, x: f32, y: f32) -> bool {
        self.inner.zoom(factor, x, y)
    }

    #[wasm_bindgen(js_name = doubleTap)]
    pub fn double_tap(&mut self, x: f32, y: f32) {
        self.inner.double_tap(x, y);
    }

    /// The scene events input queued, as JSON `[{ "event", "payload" }]`.
    #[wasm_bindgen(js_name = takeEvents)]
    pub fn take_events(&mut self) -> String {
        self.inner.take_events().to_string()
    }

    pub fn webgpu(&self) -> bool {
        self.inner.webgpu()
    }
}
