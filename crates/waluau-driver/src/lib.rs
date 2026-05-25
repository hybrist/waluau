use waluau_diagnostics::Diagnostic;

pub fn compile_source(source: &str) -> Result<Vec<u8>, Diagnostic> {
    let program = waluau_parser::parse(source)?;
    waluau_hir::type_check(&program)?;
    let ir = waluau_ir::build(&program)?;
    waluau_codegen_wasm::emit(&ir)
}

pub fn run() -> Result<(), Diagnostic> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use wasmtime::{Engine, Instance, Module, Store};

    fn fixture_source(name: &str) -> &'static str {
        match name {
            "add" => include_str!("../../../fixtures/add.walu"),
            "mismatch" => include_str!("../../../fixtures/mismatch.walu"),
            other => panic!("unknown fixture: {other}"),
        }
    }

    fn instantiate(wasm: &[u8]) -> (Store<()>, Instance) {
        let engine = Engine::default();
        let module = Module::new(&engine, wasm).expect("module should compile");
        let mut store = Store::new(&engine, ());
        let instance = Instance::new(&mut store, &module, &[]).expect("instance should create");
        (store, instance)
    }

    #[test]
    fn compiles_and_executes_valid_fixture_file() {
        let source = fixture_source("add");
        let wasm = super::compile_source(source).expect("compile should succeed");
        let (mut store, instance) = instantiate(&wasm);
        let add = instance
            .get_typed_func::<(f64, f64), f64>(&mut store, "add")
            .expect("add export should exist");
        let result = add
            .call(&mut store, (1.5, 2.25))
            .expect("call should succeed");
        assert_eq!(result, 3.75);
    }

    #[test]
    fn executes_branching_and_direct_calls() {
        let source = r#"
            fn inc(x: i32) -> i32
                return x + 1
            end

            fn max_plus_one(x: i32, y: i32) -> i32
                if x > y then
                    return inc(x)
                else
                    return inc(y)
                end
            end
        "#;
        let wasm = super::compile_source(source).expect("compile should succeed");
        let (mut store, instance) = instantiate(&wasm);
        let max_plus_one = instance
            .get_typed_func::<(i32, i32), i32>(&mut store, "max_plus_one")
            .expect("max_plus_one export should exist");
        assert_eq!(
            max_plus_one
                .call(&mut store, (7, 3))
                .expect("call should succeed"),
            8
        );
        assert_eq!(
            max_plus_one
                .call(&mut store, (2, 5))
                .expect("call should succeed"),
            6
        );
    }

    #[test]
    fn executes_loops() {
        let source = r#"
            fn sum_to(n: i32) -> i32
                let acc: i32 = 0
                let i: i32 = n
                while i > 0 do
                    acc = acc + i
                    i = i - 1
                end
                return acc
            end
        "#;
        let wasm = super::compile_source(source).expect("compile should succeed");
        let (mut store, instance) = instantiate(&wasm);
        let sum_to = instance
            .get_typed_func::<i32, i32>(&mut store, "sum_to")
            .expect("sum_to export should exist");
        assert_eq!(sum_to.call(&mut store, 5).expect("call should succeed"), 15);
    }

    #[test]
    fn rejects_invalid_fixture_file() {
        let source = fixture_source("mismatch");
        let err = super::compile_source(source).expect_err("compile should fail");
        assert_eq!(err.to_string(), "return expects f64, got bool");
    }
}
