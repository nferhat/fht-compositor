#![allow(unused_macros)]
#![allow(unused_imports)]

macro_rules! profile_function {
    () => {
        let _tracy_span = tracy_client::span!();
    };
    ($data:expr) => {
        let _location = $crate::tracy_client::span_location!();
        let _tracy_span = $crate::tracy_client::Client::running()
            .expect("function_scope! without a running tracy_client::Client")
            .span(_location, 0);
        _tracy_span.emit_text($data);
    };
}
pub(crate) use profile_function;

/// Profiling macro for feature "profile-with-puffin"
macro_rules! profile_scope {
    // Note: literal patterns provided as an optimization since they can skip an allocation.
    ($name:literal) => {
        // Note: callstack_depth is 0 since this has significant overhead
        let _tracy_span = ::tracy_client::span!($name, 0);
    };
    ($name:literal, $data:expr) => {
        // Note: callstack_depth is 0 since this has significant overhead
        let _tracy_span = ::tracy_client::span!($name, 0);
        _tracy_span.emit_text($data);
    };
    ($name:expr) => {
        let _function_name = {
            struct S;
            let type_name = core::any::type_name::<S>();
            &type_name[..type_name.len() - 3]
        };
        let _tracy_span = ::tracy_client::Client::running()
            .expect("scope! without a running tracy_client::Client")
            // Note: callstack_depth is 0 since this has significant overhead
            .span_alloc(Some($name), _function_name, file!(), line!(), 0);
    };
    ($name:expr, $data:expr) => {
        let _function_name = {
            struct S;
            let type_name = core::any::type_name::<S>();
            &type_name[..type_name.len() - 3]
        };
        let _tracy_span = ::tracy_client::Client::running()
            .expect("scope! without a running tracy_client::Client")
            // Note: callstack_depth is 0 since this has significant overhead
            .span_alloc(Some($name), _function_name, file!(), line!(), 0);
        _tracy_span.emit_text($data);
    };
}
pub(crate) use profile_scope;
