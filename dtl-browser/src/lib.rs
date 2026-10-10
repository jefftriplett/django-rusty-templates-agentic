//! Thin Wasm adapter over the shared portable template core.

pub use dtl_core::Engine;

#[cfg(target_arch = "wasm32")]
mod wasm {
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen]
    pub struct BrowserEngine {
        inner: super::Engine,
    }

    #[wasm_bindgen]
    impl BrowserEngine {
        #[wasm_bindgen(constructor)]
        pub fn new() -> Self {
            Self {
                inner: super::Engine::new(),
            }
        }

        #[wasm_bindgen(js_name = addTemplate)]
        pub fn add_template(
            &mut self,
            name: &str,
            source: &str,
        ) -> std::result::Result<(), JsValue> {
            self.inner
                .add_template(name, source)
                .map_err(|error| js_sys_error(&error))
        }

        pub fn render(
            &self,
            name: &str,
            context_json: &str,
        ) -> std::result::Result<String, JsValue> {
            self.inner
                .render(name, context_json)
                .map_err(|error| js_sys_error(&error))
        }
    }

    fn js_sys_error(error: &str) -> JsValue {
        JsValue::from_str(error)
    }
}
