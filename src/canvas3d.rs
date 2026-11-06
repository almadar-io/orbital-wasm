//! A browser 3D canvas over orbital-canvas: the page hands its `<canvas>` and each `canvas mode:"3d"` render node;
//! orbital-canvas maps, draws and answers pointer input on the device the page's `GpuHost` shares.

use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct Canvas3D {
    inner: orbital_canvas::web3d::WebCanvas3D,
}

fn button(code: u16) -> orbital_canvas::scene3d::Button {
    match code {
        1 => orbital_canvas::scene3d::Button::Middle,
        2 => orbital_canvas::scene3d::Button::Secondary,
        _ => orbital_canvas::scene3d::Button::Primary,
    }
}

#[wasm_bindgen]
impl Canvas3D {
    /// Opens (or joins) the shared device and makes a surface on `canvas`.
    pub async fn create(canvas: web_sys::HtmlCanvasElement) -> Result<Canvas3D, JsError> {
        let inner = orbital_canvas::web3d::WebCanvas3D::new(canvas)
            .await
            .map_err(|e| JsError::new(&e))?;
        Ok(Canvas3D { inner })
    }

    /// `node_json`: the `canvas` render-ui node; `theme_json`: the page's resolved scene theme.
    #[wasm_bindgen(js_name = setScene)]
    pub fn set_scene(&mut self, node_json: &str, theme_json: &str) -> Result<(), JsError> {
        let node: serde_json::Value = serde_json::from_str(node_json)
            .map_err(|e| JsError::new(&format!("node_json: {e}")))?;
        let theme: orbital_canvas::mappers::canvas_scene3d::SceneTheme =
            serde_json::from_str(theme_json)
                .map_err(|e| JsError::new(&format!("theme_json: {e}")))?;
        self.inner
            .set_scene(&node, &theme)
            .map_err(|e| JsError::new(&e))
    }

    pub fn resize(&mut self, css_width: f32, css_height: f32, pixel_ratio: f32) {
        self.inner.resize(css_width, css_height, pixel_ratio);
    }

    #[wasm_bindgen(js_name = setReducedMotion)]
    pub fn set_reduced_motion(&mut self, reduced: bool) {
        self.inner.set_reduced_motion(reduced);
    }

    pub fn render(&mut self) -> Result<(), JsError> {
        self.inner.render().map_err(|e| JsError::new(&e))
    }

    /// Advances the canvas clock to `now_ms` (`performance.now()`) and camera damping; true when the frame changed.
    pub fn frame(&mut self, now_ms: f64) -> Result<bool, JsError> {
        self.inner.frame(now_ms).map_err(|e| JsError::new(&e))
    }

    #[wasm_bindgen(js_name = pointerDown)]
    pub fn pointer_down(&mut self, id: i32, x: f32, y: f32, button_code: u16, touch: bool) {
        self.inner
            .pointer_down(id, x, y, button(button_code), touch);
    }

    #[wasm_bindgen(js_name = pointerMove)]
    pub fn pointer_move(&mut self, id: i32, x: f32, y: f32) -> bool {
        self.inner.pointer_move(id, x, y)
    }

    /// The click this release made, as JSON: `{"unit":{"id","x","z"}}`, `{"tile":{"x","z"}}`, or `null`.
    #[wasm_bindgen(js_name = pointerUp)]
    pub fn pointer_up(
        &mut self,
        id: i32,
        x: f32,
        y: f32,
        unit_events: bool,
        tile_events: bool,
    ) -> String {
        match self.inner.pointer_up(id, x, y, unit_events, tile_events) {
            Some(orbital_canvas::scene3d::Pick::Unit { id, x, z }) => {
                serde_json::json!({ "unit": { "id": id, "x": x, "z": z } }).to_string()
            }
            Some(orbital_canvas::scene3d::Pick::Tile { x, z }) => {
                serde_json::json!({ "tile": { "x": x, "z": z } }).to_string()
            }
            None => "null".into(),
        }
    }

    /// False when this canvas runs on the WebGL2 fallback (no GPU drawables there).
    pub fn webgpu(&self) -> bool {
        self.inner.webgpu()
    }

    pub fn wheel(&mut self, delta_y: f32) -> bool {
        self.inner.wheel(delta_y)
    }

    /// The camera as the user moved it, as JSON (`eye`, `target`, `up`, `projection`, `near`, `far`).
    pub fn camera(&self) -> String {
        self.inner.camera().map_or_else(
            || "null".into(),
            |c| serde_json::to_string(&c).unwrap_or_else(|_| "null".into()),
        )
    }
}
