//! The resource every span and log record is reported under.
//!
//! One resource, built once, for both the Rust side and the webview: a span
//! the webview exports is re-stamped with it rather than carrying one of its
//! own, so the two halves of the app can never report two identities.
//!
//! Every key is an OpenTelemetry semantic-convention key. The ones that decide
//! whether a record can be told apart from another build's are **required**
//! by [`crate::Builder::new`] or read from the installed artifact, never
//! defaulted, because a silent default is how a build comes to report the
//! wrong identity without anyone noticing.

use opentelemetry::KeyValue;
use opentelemetry_sdk::Resource;
use opentelemetry_semantic_conventions::attribute as semconv;

/// What only the running app can say about itself.
#[derive(Debug, Clone)]
pub(crate) struct Identity {
    pub service_name: String,
    pub deployment_environment_name: String,
    /// The version of the *installed artifact*: Tauri's `PackageInfo`, which is
    /// `tauri.conf.json`'s `version` (or the crate's) compiled into the binary.
    pub service_version: String,
    pub build_id: Option<String>,
    pub extra: Vec<KeyValue>,
}

impl Identity {
    pub(crate) fn resource(&self) -> Resource {
        let mut attributes = vec![
            KeyValue::new(semconv::SERVICE_VERSION, self.service_version.clone()),
            KeyValue::new(
                semconv::DEPLOYMENT_ENVIRONMENT_NAME,
                self.deployment_environment_name.clone(),
            ),
            KeyValue::new(semconv::SERVICE_INSTANCE_ID, instance_id()),
            KeyValue::new(semconv::OS_TYPE, os_type(std::env::consts::OS)),
            KeyValue::new(semconv::HOST_ARCH, host_arch(std::env::consts::ARCH)),
            KeyValue::new(semconv::TELEMETRY_DISTRO_NAME, env!("CARGO_PKG_NAME")),
            KeyValue::new(semconv::TELEMETRY_DISTRO_VERSION, env!("CARGO_PKG_VERSION")),
        ];
        let os = os_info::get();
        attributes.push(KeyValue::new(semconv::OS_NAME, os.os_type().to_string()));
        if *os.version() != os_info::Version::Unknown {
            attributes.push(KeyValue::new(semconv::OS_VERSION, os.version().to_string()));
        }
        if let Some(build_id) = &self.build_id {
            attributes.push(KeyValue::new(semconv::APP_BUILD_ID, build_id.clone()));
        }
        // Last, so an app that knows better than the platform probe wins the
        // merge for any key it sets.
        attributes.extend(self.extra.iter().cloned());
        Resource::builder()
            .with_service_name(self.service_name.clone())
            .with_attributes(attributes)
            .build()
    }
}

/// `os.type`, whose values the semantic conventions enumerate. They name the
/// kernel family, not the product: iOS is `darwin`, Android is `linux`, and
/// the product goes in `os.name`.
///
/// ```
/// # use tauri_plugin_otel::__doc::os_type;
/// assert_eq!(os_type("macos"), "darwin");
/// assert_eq!(os_type("ios"), "darwin");
/// assert_eq!(os_type("android"), "linux");
/// assert_eq!(os_type("windows"), "windows");
/// ```
pub fn os_type(rust_os: &str) -> &str {
    match rust_os {
        "macos" | "ios" | "tvos" | "watchos" | "visionos" => "darwin",
        "android" => "linux",
        "dragonfly" => "dragonflybsd",
        "illumos" => "solaris",
        other => other,
    }
}

/// `host.arch`, whose values the semantic conventions enumerate and which
/// differ from Rust's own spelling for the two that matter most.
///
/// ```
/// # use tauri_plugin_otel::__doc::host_arch;
/// assert_eq!(host_arch("x86_64"), "amd64");
/// assert_eq!(host_arch("aarch64"), "arm64");
/// assert_eq!(host_arch("arm"), "arm32");
/// ```
pub fn host_arch(rust_arch: &str) -> &str {
    match rust_arch {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "arm" => "arm32",
        "powerpc" => "ppc32",
        "powerpc64" => "ppc64",
        other => other,
    }
}

/// `service.instance.id`: random per process, as the conventions recommend for
/// a service with no stable instance name. Two windows of one app share it;
/// two launches do not.
fn instance_id() -> String {
    let mut bytes = [0_u8; 16];
    if getrandom::fill(&mut bytes).is_err() {
        // No entropy is no reason to lose telemetry; a time-derived id is
        // still unique enough to separate two launches.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        bytes = nanos.to_le_bytes();
    }
    // A version-4 UUID's layout, so a backend that parses the id as one can.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> Identity {
        Identity {
            service_name: "vaam-vendor".into(),
            deployment_environment_name: "production".into(),
            service_version: "1.4.2".into(),
            build_id: Some("1.4.2+812".into()),
            extra: vec![KeyValue::new("vaam.product", "vendor")],
        }
    }

    fn get(resource: &Resource, key: &'static str) -> Option<String> {
        resource
            .get(&opentelemetry::Key::from_static_str(key))
            .map(|value| value.to_string())
    }

    #[test]
    fn the_resource_carries_every_identifying_key() {
        let resource = identity().resource();
        assert_eq!(
            get(&resource, semconv::SERVICE_NAME).as_deref(),
            Some("vaam-vendor")
        );
        assert_eq!(
            get(&resource, semconv::SERVICE_VERSION).as_deref(),
            Some("1.4.2")
        );
        assert_eq!(
            get(&resource, semconv::DEPLOYMENT_ENVIRONMENT_NAME).as_deref(),
            Some("production")
        );
        assert_eq!(
            get(&resource, semconv::APP_BUILD_ID).as_deref(),
            Some("1.4.2+812")
        );
        assert_eq!(get(&resource, "vaam.product").as_deref(), Some("vendor"));
        for key in [
            semconv::SERVICE_INSTANCE_ID,
            semconv::OS_TYPE,
            semconv::OS_NAME,
            semconv::HOST_ARCH,
            semconv::TELEMETRY_DISTRO_NAME,
            semconv::TELEMETRY_SDK_NAME,
        ] {
            assert!(get(&resource, key).is_some(), "{key} is missing");
        }
    }

    /// The deprecated two-segment key must never appear next to the stable one.
    #[test]
    fn the_deprecated_environment_key_is_not_emitted() {
        assert!(get(&identity().resource(), "deployment.environment").is_none());
    }

    #[test]
    fn no_build_id_means_no_build_id_key() {
        let resource = Identity {
            build_id: None,
            ..identity()
        }
        .resource();
        assert!(get(&resource, semconv::APP_BUILD_ID).is_none());
    }

    #[test]
    fn an_app_attribute_overrides_the_platform_probe() {
        let resource = Identity {
            extra: vec![KeyValue::new(semconv::OS_NAME, "custom")],
            ..identity()
        }
        .resource();
        assert_eq!(get(&resource, semconv::OS_NAME).as_deref(), Some("custom"));
    }

    #[test]
    fn instance_ids_are_uuid_shaped_and_distinct() {
        let (a, b) = (instance_id(), instance_id());
        assert_ne!(a, b);
        assert_eq!(a.len(), 36);
        assert_eq!(&a[14..15], "4");
    }
}
