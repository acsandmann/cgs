macro_rules! native_control {
    ($wrapper:ident, $native:ident, $accessor:ident) => {
        impl $wrapper {
            pub fn $accessor(&self) -> &$native { &self.native }
        }
        impl NativeView for $wrapper {
            fn ns_view(&self) -> &NSView { &self.native }
        }
        impl NativeControl for $wrapper {
            fn ns_control(&self) -> &NSControl { &self.native }
        }
    };
}
pub(crate) use native_control;
