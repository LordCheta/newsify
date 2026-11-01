//! Clock and randomness from the Workers runtime.

use newsify_core::Platform;
use worker::js_sys;
use worker::wasm_bindgen::{JsCast, JsValue};

pub struct WorkerPlatform;

impl Platform for WorkerPlatform {
    fn now_secs(&self) -> i64 {
        (js_sys::Date::now() / 1000.0) as i64
    }

    fn random_bytes(&self, n: usize) -> Vec<u8> {
        // `crypto.getRandomValues` is available on the Workers global.
        let global = js_sys::global();
        let crypto = js_sys::Reflect::get(&global, &JsValue::from_str("crypto"))
            .expect("Workers provide a global `crypto`");
        let get_random_values =
            js_sys::Reflect::get(&crypto, &JsValue::from_str("getRandomValues"))
                .expect("crypto.getRandomValues exists")
                .dyn_into::<js_sys::Function>()
                .expect("getRandomValues is a function");
        let array = js_sys::Uint8Array::new_with_length(n as u32);
        get_random_values
            .call1(&crypto, &array)
            .expect("crypto.getRandomValues succeeds");
        array.to_vec()
    }
}
