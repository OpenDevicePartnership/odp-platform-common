#[cfg(any(feature = "acpi", feature = "serial"))]
pub(crate) mod guid {
    pub const _SENSOR_CRT_TEMP: uuid::Uuid = uuid::uuid!("218246e7-baf6-45f1-aa13-07e4845256b8");
    pub const _SENSOR_PROCHOT_TEMP: uuid::Uuid = uuid::uuid!("22dc52d2-fd0b-47ab-95b8-26552f9831a5");
    pub const FAN_ON_TEMP: uuid::Uuid = uuid::uuid!("ba17b567-c368-48d5-bc6f-a312a41583c1");
    pub const FAN_RAMP_TEMP: uuid::Uuid = uuid::uuid!("3a62688c-d95b-4d2d-bacc-90d7a5816bcd");
    pub const FAN_MAX_TEMP: uuid::Uuid = uuid::uuid!("dcb758b1-f0fd-4ec7-b2c0-ef1e2a547b76");
    pub const FAN_MIN_RPM: uuid::Uuid = uuid::uuid!("db261c77-934b-45e2-9742-256c62badb7a");
    pub const FAN_MAX_RPM: uuid::Uuid = uuid::uuid!("5cf839df-8be7-42b9-9ac5-3403ca2c8a6a");
    pub const FAN_CURRENT_RPM: uuid::Uuid = uuid::uuid!("adf95492-0776-4ffc-84f3-b6c8b5269683");
}

/// Convert deciKelvin to degrees Celsius
#[cfg(any(feature = "mock", feature = "acpi", feature = "serial"))]
pub(crate) const fn dk_to_c(dk: u32) -> f64 {
    (dk as f64 / 10.0) - 273.15
}
