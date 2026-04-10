// TODO: Remove these wrapper types once a version of embedded-batteries is published
// containing `Debug` implementations for these types.
//
// They have been upstreamed, but are currently not published to crates.io.

pub struct DebugBstReturn<'a>(pub &'a battery_service_messages::BstReturn);

impl std::fmt::Debug for DebugBstReturn<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BstReturn")
            .field("battery_state", &self.0.battery_state.bits())
            .field("battery_present_rate", &self.0.battery_present_rate)
            .field("battery_remaining_capacity", &self.0.battery_remaining_capacity)
            .field("battery_present_voltage", &self.0.battery_present_voltage)
            .finish()
    }
}

pub struct DebugBixFixedStrings<'a>(pub &'a battery_service_messages::BixFixedStrings);

fn str_from_bytes(bytes: &[u8]) -> &str {
    core::ffi::CStr::from_bytes_until_nul(bytes)
        .ok()
        .and_then(|c| c.to_str().ok())
        .unwrap_or("Invalid")
}

impl std::fmt::Debug for DebugBixFixedStrings<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BixFixedStrings")
            .field("revision", &self.0.revision)
            .field("power_unit", &(self.0.power_unit as u32))
            .field("design_capacity", &self.0.design_capacity)
            .field("last_full_charge_capacity", &self.0.last_full_charge_capacity)
            .field("battery_technology", &(self.0.battery_technology as u32))
            .field("design_voltage", &self.0.design_voltage)
            .field("design_cap_of_warning", &self.0.design_cap_of_warning)
            .field("design_cap_of_low", &self.0.design_cap_of_low)
            .field("cycle_count", &self.0.cycle_count)
            .field("measurement_accuracy", &self.0.measurement_accuracy)
            .field("max_sampling_time", &self.0.max_sampling_time)
            .field("min_sampling_time", &self.0.min_sampling_time)
            .field("max_averaging_interval", &self.0.max_averaging_interval)
            .field("min_averaging_interval", &self.0.min_averaging_interval)
            .field("battery_capacity_granularity_1", &self.0.battery_capacity_granularity_1)
            .field("battery_capacity_granularity_2", &self.0.battery_capacity_granularity_2)
            .field("model_number", &str_from_bytes(&self.0.model_number))
            .field("serial_number", &str_from_bytes(&self.0.serial_number))
            .field("battery_type", &str_from_bytes(&self.0.battery_type))
            .field("oem_info", &str_from_bytes(&self.0.oem_info))
            .field(
                "battery_swapping_capability",
                &(self.0.battery_swapping_capability as u32),
            )
            .finish()
    }
}
