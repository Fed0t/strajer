use crate::LanError;

const WARCRAFT_SERVICE_TYPE: &str = "_blizzard._udp";

pub fn service_registration_type(version: &str) -> Result<String, LanError> {
    let components: Vec<&str> = version.split('.').collect();
    if components.len() != 4 {
        return Err(LanError::UnsupportedWarcraftVersion(version.to_owned()));
    }

    let major = parse_version_component(components[0], version)?;
    let minor = parse_version_component(components[1], version)?;
    parse_version_component(components[2], version)?;
    parse_version_component(components[3], version)?;

    let major_offset = major
        .checked_sub(1)
        .ok_or_else(|| LanError::UnsupportedWarcraftVersion(version.to_owned()))?;

    if minor > 99 {
        return Err(LanError::UnsupportedWarcraftVersion(version.to_owned()));
    }

    let decimal_discriminator = format!("10{major_offset}{minor:02}")
        .parse::<u64>()
        .map_err(|_| LanError::UnsupportedWarcraftVersion(version.to_owned()))?;

    Ok(format!(
        "{WARCRAFT_SERVICE_TYPE},_w3xp{decimal_discriminator:x}"
    ))
}

fn parse_version_component(component: &str, version: &str) -> Result<u64, LanError> {
    if component.is_empty() || !component.bytes().all(is_ascii_digit) {
        return Err(LanError::UnsupportedWarcraftVersion(version.to_owned()));
    }

    component
        .parse::<u64>()
        .map_err(|_| LanError::UnsupportedWarcraftVersion(version.to_owned()))
}

fn is_ascii_digit(byte: u8) -> bool {
    byte.is_ascii_digit()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_known_warcraft_service_types() {
        assert_eq!(
            service_registration_type("1.33.0.00000").expect("version should be supported"),
            "_blizzard._udp,_w3xp2731"
        );
        assert_eq!(
            service_registration_type("1.34.0.00000").expect("version should be supported"),
            "_blizzard._udp,_w3xp2732"
        );
        assert_eq!(
            service_registration_type("2.0.4.23745").expect("version should be supported"),
            "_blizzard._udp,_w3xp2774"
        );
    }

    #[test]
    fn rejects_malformed_versions() {
        assert!(service_registration_type("2.0.4").is_err());
        assert!(service_registration_type("zero.0.4.23745").is_err());
        assert!(service_registration_type("0.0.4.23745").is_err());
    }
}
