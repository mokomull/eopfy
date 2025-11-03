use std::{
    collections::HashMap,
    path::PathBuf,
    process::Command,
    time::{Duration, SystemTime},
};

use anyhow::Context;
use handlebars::Handlebars;
use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use serde_xml_rs::SerdeXml;
use uuid::Uuid;
use virt::{
    domain::Domain,
    error::ErrorNumber,
    sys::{
        VIR_CONNECT_LIST_DOMAINS_TRANSIENT, VIR_DOMAIN_AFFECT_CURRENT, VIR_DOMAIN_AFFECT_LIVE,
        VIR_DOMAIN_METADATA_ELEMENT, VIR_DOMAIN_NONE,
    },
};
use xml::EmitterConfig;

static DOMAIN_TEMPLATE_NAME: &str = "DOMAIN";
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
    #[serde(
        deserialize_with = "super::duration_humantime",
        default = "default_vm_duration"
    )]
    vm_duration: Duration,
}

fn default_vm_duration() -> Duration {
    Duration::from_secs(900)
}

pub struct Libvirt {
    connection: virt::connect::Connect,
    template: Handlebars<'static>,
    config: Config,
}

impl Libvirt {
    pub fn connect(config: Config) -> anyhow::Result<Self> {
        let connection = virt::connect::Connect::open(Some(&config.connection_string))
            .context("connecting to libvirt")?;

        let mut handlebars = Handlebars::new();
        handlebars
            .register_template_file(DOMAIN_TEMPLATE_NAME, &config.xml_template_path)
            .context("registering XML template")?;

        Ok(Self {
            config,
            connection,
            template: handlebars,
        })
    }

    fn get_domain(&mut self, uuid: Uuid) -> anyhow::Result<Option<Domain>> {
        match Domain::lookup_by_uuid(&self.connection, uuid) {
            Ok(d) => Ok(Some(d)),
            Err(e) if e.code() == ErrorNumber::NoDomain => Ok(None),
            Err(e) => Err(anyhow::Error::from(e).context("lookup_by_uuid")),
        }
    }

    pub fn get_expiration_for(&mut self, uuid: Uuid) -> anyhow::Result<Option<SystemTime>> {
        let domain = self.get_domain(uuid)?;
        let Some(domain) = domain else {
            return Ok(None);
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

    fn keepalive(&mut self, domain: Domain) -> anyhow::Result<()> {
        let metadata = Metadata {
            expiration: SystemTime::now() + self.config.vm_duration,
        };
        domain
            .set_metadata(
                VIR_DOMAIN_METADATA_ELEMENT as i32,
                Some(
                    &metadata
                        .to_libvirt()
                        .expect("metadata serialization should never fail"),
                ),
                Some("eopfy"),
                Some(XML_NAMESPACE),
                VIR_DOMAIN_AFFECT_LIVE,
            )
            .context("set_metadata")?;
        Ok(())
    }

    fn create(&mut self, uuid: Uuid) -> anyhow::Result<String> {
        let disk = tempfile::NamedTempFile::new_in(&self.config.temporary_dir)?;

        // create the qcow2 image
        let status = Command::new("/usr/bin/qemu-img")
            .arg("create")
            .arg("-b")
            .arg(&self.config.disk_template_path)
            .arg("-F")
            .arg("qcow2")
            .arg("-f")
            .arg("qcow2")
            .arg(disk.path())
            .spawn()
            .context("spawning qemu-img")?
            .wait()
            .context("wait failed for some reason")?; // TODO: can this even happen?
        if !status.success() {
            anyhow::bail!("qemu-img failed with error {:?}", status.code());
        }

        let status = Command::new("/usr/bin/setfacl")
            .arg("-m")
            .arg("u:libvirt-qemu:rw")
            .arg(disk.path())
            .spawn()
            .context("spawning setfacl")?
            .wait()
            .context("why would wait fail")?;
        if !status.success() {
            anyhow::bail!("setfacl failed with error {:?}", status.code());
        }

        let uuid = uuid.to_string();
        let name = format!("temporary-{}", uuid);
        let socket_relative_path = format!("vnc-{}", uuid);
        let vnc_unix_socket = format!("{}/{}", self.config.temporary_dir, socket_relative_path);

        // template the XML
        let domain_xml = self
            .template
            .render(
                DOMAIN_TEMPLATE_NAME,
                &HashMap::from([
                    ("name", name.as_str()),
                    ("uuid", &uuid),
                    (
                        "disk",
                        disk.path()
                            .to_str()
                            .expect("NamedTempFile paths should always be UTF-8"),
                    ),
                    (
                        "metadata",
                        Metadata {
                            expiration: SystemTime::now() + self.config.vm_duration,
                        }
                        .to_libvirt()
                        .expect("metadata serialization should be infallible")
                        .as_str(),
                    ),
                    ("vnc_unix_socket", &vnc_unix_socket),
                ]),
            )
            .context("creating domain template")?;

        Domain::create_xml(&self.connection, &domain_xml, VIR_DOMAIN_NONE)
            .context("launching VM")?;

        Ok(vnc_unix_socket)
    }

    pub fn create_or_keepalive(&mut self, uuid: Uuid) -> anyhow::Result<String> {
        if let Some(domain) = self.get_domain(uuid)? {
            self.keepalive(domain)?;
            Ok(format!("{}/vnc-{}", self.config.temporary_dir, uuid))
        } else {
            // this is long-running so it should run with spawn_blocking, but ... this API
            // intentionally takes a &mut self so that no concurrent mutations can happen so it
            // really doesn't matter if I break a tokio runner thread.
            self.create(uuid)
        }
    }

    pub fn expire(&mut self) -> anyhow::Result<()> {
        let now = SystemTime::now();

        for domain in self
            .connection
            .list_all_domains(VIR_CONNECT_LIST_DOMAINS_TRANSIENT)
            .context("list_all_domains")?
        {
            info!("checking domain {}", domain.get_uuid().unwrap());
            let metadata = match domain.get_metadata(
                VIR_DOMAIN_METADATA_ELEMENT as i32,
                Some(XML_NAMESPACE),
                0,
            ) {
                Ok(s) => s,
                Err(e) if e.code() == ErrorNumber::NoDomainMetadata => {
                    warn!("this domain has no eopfy metadata");
                    continue;
                }
                Err(e) => return Err(anyhow::Error::from(e).context("get_metadata")),
            };

            let metadata = match Metadata::from_libvirt(&metadata) {
                Ok(m) => m,
                Err(e) => {
                    warn!("could not parse eopfy metadata: {e:?}");
                    continue;
                }
            };

            if metadata.expiration < now {
                info!("stopping the domain");
                if let Err(e) = domain.destroy() {
                    error!("could not destroy the domain: {e:?}");
                }
            }
        }

        Ok(())
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
