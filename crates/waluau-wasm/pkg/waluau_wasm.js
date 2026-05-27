import * as wasm from "./waluau_wasm_bg.wasm";
import { __wbg_set_wasm } from "./waluau_wasm_bg.js";

__wbg_set_wasm(wasm);
wasm.__wbindgen_start();
export {
    compile
} from "./waluau_wasm_bg.js";
