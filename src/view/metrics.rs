pub struct Metrics;
impl Metrics {
    pub const CONTROL_SPACING: f64 = 8.0;
    pub const DETAIL_MIN_WIDTH: f64 = 400.0;
    pub const PAGE_INSET: f64 = 20.0;
    pub const PAGE_TOP_INSET: f64 = 20.0;
    /// The shortest row in a grouped settings card, so rows read evenly whatever their control.
    pub const ROW_MIN_HEIGHT: f64 = 22.0;
    pub const ROW_SPACING: f64 = 6.0;
    pub const SECTION_SPACING: f64 = 16.0;
    pub const SIDEBAR_MIN_WIDTH: f64 = 160.0;
    pub const SIDEBAR_WIDTH: f64 = 200.0;
    /// A continuous slider beside a value field, as in System Settings.
    pub const SLIDER_WIDTH: f64 = 160.0;
    /// A short numeric field with its unit inside the bezel.
    pub const VALUE_FIELD_WIDTH: f64 = 60.0;
}
