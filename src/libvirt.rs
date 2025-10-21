use std::{path::PathBuf, time::SystemTime};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use serde_xml_rs::SerdeXml;
use uuid::Uuid;
use virt::{
    domain::Domain,
    error::ErrorNumber,
    sys::{VIR_DOMAIN_AFFECT_CURRENT, VIR_DOMAIN_METADATA_ELEMENT},
};
use xml::EmitterConfig;

static XML_NAMESPACE: &str = "https://eopfy.mmlx.us/metadata";

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename = "metadata")]
struct Metadata {
    expiration: SystemTime,
}

impl Metadata {
    fn from_libvirt(data: &str) -> anyhow::Result<Self> {
        let serde_xml = SerdeXml::new();
        serde_xml.from_str(data).map_err(Into::into)
    }

    fn to_libvirt(&self) -> anyhow::Result<String> {
        let emitter = EmitterConfig::new().write_document_declaration(false);
        let serde_xml = SerdeXml::new()
            .default_namespace(XML_NAMESPACE)
            .emitter(emitter);
        serde_xml.to_string(self).map_err(Into::into)
    }
}

#[derive(Deserialize)]
pub struct Config {
    connection_string: String,
    xml_template_path: PathBuf,
    disk_template_path: PathBuf,
    temporary_dir: String,
}

pub struct Libvirt {
    connection: virt::connect::Connect,
    config: Config,
}

impl Libvirt {
    pub fn connect(config: Config) -> anyhow::Result<Self> {
        let connection = virt::connect::Connect::open(Some(&config.connection_string))
            .context("connecting to libvirt")?;
        Ok(Self { config, connection })
    }

    pub fn get_expiration_for(&mut self, uuid: Uuid) -> anyhow::Result<Option<SystemTime>> {
        let domain = match Domain::lookup_by_uuid(&self.connection, uuid) {
            Ok(d) => d,
            Err(e) if e.code() == ErrorNumber::NoDomain => return Ok(None),
            Err(e) => return Err(anyhow::Error::from(e).context("lookup_by_uuid")),
        };
        let metadata = domain
            .get_metadata(
                VIR_DOMAIN_METADATA_ELEMENT as i32,
                Some(XML_NAMESPACE),
                VIR_DOMAIN_AFFECT_CURRENT,
            )
            .context("get_metadata")?;
        let metadata = Metadata::from_libvirt(&metadata).context("parsing metadata")?;
        Ok(Some(metadata.expiration))
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    #[allow(non_snake_case)]
    fn deserialize_from_libvirt_virDomainGetMetadata() {
        // libvirt seems to clean up the namespaces, since you ask for metadata by xmlns
        let text = "<metadata><expiration><secs_since_epoch>0</secs_since_epoch><nanos_since_epoch>0</nanos_since_epoch></expiration></metadata>";
        assert_eq!(
            Metadata::from_libvirt(text).unwrap(),
            Metadata {
                expiration: SystemTime::UNIX_EPOCH,
            }
        );
    }

    #[test]
    fn serialize_to_libvirt() {
        let metadata = Metadata {
            expiration: SystemTime::UNIX_EPOCH,
        };
        assert_eq!(
            &metadata.to_libvirt().unwrap(),
            "<metadata xmlns=\"https://eopfy.mmlx.us/metadata\"><expiration><secs_since_epoch>0</secs_since_epoch><nanos_since_epoch>0</nanos_since_epoch></expiration></metadata>"
        );
    }
}
