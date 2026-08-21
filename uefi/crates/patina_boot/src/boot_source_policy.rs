//! Explicit policy for boot sources discovered outside provisioned `Boot####` entries.

extern crate alloc;

use alloc::vec::Vec;

use patina::device_path::{
    node_defs::{DevicePathType, HardDrive, MessagingSubType},
    paths::{DevicePath, DevicePathBuf},
};

/// Firmware Secure Boot state used to authorize fallback boot sources.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecureBootState {
    /// Secure Boot is enabled and the platform is not in setup mode.
    Enabled,
    /// Secure Boot is disabled and the platform is not in setup mode.
    Disabled,
    /// The platform is provisioning keys in setup mode.
    SetupMode,
}

/// OEM policy for filesystem fallback after provisioned boot options fail.
///
/// The default policy permits no fallback sources. Approved internal volume
/// paths, removable media, and non-enforcing Secure Boot states must each be
/// enabled explicitly.
#[derive(Debug, Clone, Default)]
pub struct BootSourcePolicy {
    approved_internal_devices: Vec<DevicePathBuf>,
    allow_removable_media: bool,
    allow_secure_boot_disabled: bool,
    allow_setup_mode: bool,
}

impl BootSourcePolicy {
    /// Create a policy with no fallback sources enabled.
    pub fn new() -> Self {
        Self::default()
    }

    /// Approve an internal filesystem volume for fallback.
    ///
    /// `device_path` must contain a HardDrive partition node and may end there
    /// or include the full volume path. USB paths are never treated as
    /// internal; enable them separately with [`Self::allow_removable_media`].
    pub fn with_approved_internal_device(mut self, device_path: DevicePathBuf) -> Self {
        self.approved_internal_devices.push(device_path);
        self
    }

    /// Permit fallback to removable USB filesystem volumes.
    pub fn allow_removable_media(mut self) -> Self {
        self.allow_removable_media = true;
        self
    }

    /// Permit fallback while Secure Boot is disabled outside setup mode.
    pub fn allow_when_secure_boot_disabled(mut self) -> Self {
        self.allow_secure_boot_disabled = true;
        self
    }

    /// Permit fallback while the platform is in Secure Boot setup mode.
    pub fn allow_in_setup_mode(mut self) -> Self {
        self.allow_setup_mode = true;
        self
    }

    pub(crate) fn has_sources(&self) -> bool {
        self.allow_removable_media || !self.approved_internal_devices.is_empty()
    }

    pub(crate) fn allows_security_state(&self, state: SecureBootState) -> bool {
        match state {
            SecureBootState::Enabled => true,
            SecureBootState::Disabled => self.allow_secure_boot_disabled,
            SecureBootState::SetupMode => self.allow_setup_mode,
        }
    }

    pub(crate) fn allows_device(&self, candidate: &DevicePath) -> bool {
        if is_usb_device_path(candidate) {
            return self.allow_removable_media;
        }

        self.approved_internal_devices.iter().any(|approved| {
            approved
                .as_ref()
                .iter()
                .any(|node| HardDrive::try_from_node(&node).is_some())
                && device_path_has_prefix(candidate, approved.as_ref())
        })
    }
}

fn is_usb_device_path(device_path: &DevicePath) -> bool {
    device_path.iter().any(|node| {
        node.header.r#type == DevicePathType::Messaging as u8
            && matches!(
                node.header.sub_type,
                value if value == MessagingSubType::Usb as u8
                    || value == MessagingSubType::UsbClass as u8
                    || value == MessagingSubType::UsbWwid as u8
            )
    })
}

fn device_path_has_prefix(candidate: &DevicePath, approved: &DevicePath) -> bool {
    let approved_bytes = without_end_entire(approved.as_bytes());
    !approved_bytes.is_empty() && candidate.as_bytes().starts_with(approved_bytes)
}

fn without_end_entire(bytes: &[u8]) -> &[u8] {
    const END_ENTIRE_SIZE: usize = 4;
    const END_TYPE: u8 = 0x7f;
    const END_SUBTYPE_ENTIRE: u8 = 0xff;

    if bytes.len() >= END_ENTIRE_SIZE
        && bytes[bytes.len() - END_ENTIRE_SIZE] == END_TYPE
        && bytes[bytes.len() - END_ENTIRE_SIZE + 1] == END_SUBTYPE_ENTIRE
    {
        &bytes[..bytes.len() - END_ENTIRE_SIZE]
    } else {
        bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use patina::device_path::node_defs::{Acpi, Pci};

    fn internal_path(root: u32) -> DevicePathBuf {
        let mut path = DevicePathBuf::from_device_path_node_iter([Acpi::new_pci_root(root)].into_iter());
        let pci = DevicePathBuf::from_device_path_node_iter([Pci { function: 0, device: 1 }].into_iter());
        path.append_device_path(&pci);
        let partition = DevicePathBuf::from_device_path_node_iter(
            [HardDrive::new_gpt(1, 2048, 1_000_000, [root as u8; 16])].into_iter(),
        );
        path.append_device_path(&partition);
        path
    }

    fn usb_path() -> DevicePathBuf {
        let bytes = [0x03, 0x05, 0x06, 0x00, 0x00, 0x00, 0x7f, 0xff, 0x04, 0x00];
        // SAFETY: `bytes` contains a valid USB node followed by EndEntire.
        let path = unsafe { DevicePath::try_from_ptr(bytes.as_ptr()) }.unwrap();
        DevicePathBuf::from(path)
    }

    #[test]
    fn default_policy_denies_all_sources() {
        let policy = BootSourcePolicy::new();
        assert!(!policy.has_sources());
        assert!(!policy.allows_device(internal_path(0).as_ref()));
    }

    #[test]
    fn approved_internal_prefix_matches_only_that_device() {
        let approved = internal_path(0);
        let policy = BootSourcePolicy::new().with_approved_internal_device(approved.clone());

        assert!(policy.allows_device(approved.as_ref()));
        assert!(!policy.allows_device(internal_path(1).as_ref()));
    }

    #[test]
    fn broad_controller_prefix_is_not_an_approved_volume() {
        let controller = DevicePathBuf::from_device_path_node_iter([Acpi::new_pci_root(0)].into_iter());
        let candidate = internal_path(0);
        let policy = BootSourcePolicy::new().with_approved_internal_device(controller);

        assert!(!policy.allows_device(candidate.as_ref()));
    }

    #[test]
    fn removable_media_requires_separate_opt_in() {
        let usb = usb_path();
        let internal_only = BootSourcePolicy::new().with_approved_internal_device(usb.clone());
        assert!(!internal_only.allows_device(usb.as_ref()));

        let removable = internal_only.allow_removable_media();
        assert!(removable.allows_device(usb.as_ref()));
    }

    #[test]
    fn non_enforcing_security_states_require_opt_in() {
        let default = BootSourcePolicy::new();
        assert!(default.allows_security_state(SecureBootState::Enabled));
        assert!(!default.allows_security_state(SecureBootState::Disabled));
        assert!(!default.allows_security_state(SecureBootState::SetupMode));

        let permissive = default.allow_when_secure_boot_disabled().allow_in_setup_mode();
        assert!(permissive.allows_security_state(SecureBootState::Disabled));
        assert!(permissive.allows_security_state(SecureBootState::SetupMode));
    }
}
