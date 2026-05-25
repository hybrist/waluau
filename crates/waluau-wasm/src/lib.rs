use std::ffi::CString;
use std::os::raw::c_char;

#[unsafe(no_mangle)]
pub extern "C" fn alloc(size: usize) -> *mut u8 {
    let mut buf = Vec::with_capacity(size);
    let ptr = buf.as_mut_ptr();
    std::mem::forget(buf);
    ptr
}

/// # Safety
///
/// Deallocates the memory previously allocated by `alloc`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dealloc(ptr: *mut u8, size: usize) {
    unsafe {
        let _ = Vec::from_raw_parts(ptr, size, size);
    }
}

/// # Safety
///
/// Frees the C string returned by `compile`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn free_string(ptr: *mut c_char) {
    if !ptr.is_null() {
        unsafe {
            let _ = CString::from_raw(ptr);
        }
    }
}

/// # Safety
///
/// Compiles the compiler source string of length `len` starting at `ptr`.
/// The caller must ensure that the pointer points to `len` bytes of valid memory.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn compile(ptr: *const u8, len: usize) -> *mut c_char {
    let slice = unsafe { std::slice::from_raw_parts(ptr, len) };
    let source = match std::str::from_utf8(slice) {
        Ok(s) => s,
        Err(_) => return CString::new("Error: Invalid UTF-8").unwrap().into_raw(),
    };

    let result = match compile_source_to_string(source) {
        Ok(output) => format!("Success:\n{}", output),
        Err(err) => format!("Error:\n{}", err),
    };

    CString::new(result).unwrap().into_raw()
}

fn compile_source_to_string(source: &str) -> Result<String, String> {
    let program = waluau_parser::parse(source).map_err(|e| e.to_string())?;
    waluau_hir::type_check(&program).map_err(|e| e.to_string())?;
    let module = waluau_ir::build(&program).map_err(|e| e.to_string())?;

    let mut output = String::new();
    for function in &module.functions {
        output.push_str(&function.dump());
        output.push('\n');
    }
    Ok(output)
}
