# eopfy

eopfy (pronounced "eepy" because it puts the VMs down for a nap) manages temporary VMs with
libvirt+qemu and terminates them if the user abandons it for too long.

# Synopsis

    cargo run --release -- config.toml

# Description

eopfy manages temporary VMs via libvirt and serves a web interface to them thanks to the [novnc]
project.

When a VM is requested, it creates a new qcow2 image that references `disk_template_path` as a
backing store.  It then creates a transient libvirt domain based on a Handlebars template at
`xml_template_path`.

Some delay (by default 15 minutes) after the last time a VM has been connected-to, the VM will be
terminated.

Each VM console is available via VNC at a UNIX socket at `{{temporary_dir}}/vnc-{{uuid of the VM}}`.

The VNC sockets should be served to the client using the `websockify` tool:

    $ websockify --token-plugin="UnixDomainSocketDirectory" --token-source=/tmp/eopfy [::]:5000

# Configuration

`config.toml` should mostly speak for itself.

The `websocket_uri` parameter will have `?token={{ basename of weboscket path }}` applied.  This
assumes that websockify is running with the correct `--token-source`.

The domain template XML is a Handlebars template where

  * `name` is the name of the VM (e.g. `temporary-{uuid}`)
  * `uuid` is the UUID of the VM.  It's currently exactly the same as the axum_session Session's
    UUID.
  * `metadata` is a raw blob of XML for the medatada that eopfy is storing inside libvirt's domain
    configuration.  You probably want to use `{{{`-strings with Handlebars for this.
  * `disk` is the path to a temporary file created for this VMs' C:\ drive.  It will be
    `unlink(2)`ed as soon as the
  * `vnc_unix_socket` is the path to the UNIX socket that qemu will listen for VNC connections. This
    is currently a path within `temporary_dir`.

# Examples

## config.toml

    static_web_path = "../web/dist"
    cookie_key = "[redacted.  head -c 64 /dev/urandom | base64]"
    listen_address = "[::]:9000"
    websocket_uri = "wss://publicly-accessible-hostname/websockify"

    [libvirt]
    connection_string = "qemu:///system"
    xml_template_path = "domain_template.xml"
    disk_template_path = "disk.img"
    temporary_dir = "/tmp/eopfy"

# Instructions to future self

Some sort of working npm environment is needed to build `web/`.  I got mine at NodeSource, because
Ubuntu 22.04's is too old to understand how to behave in an IPv6-only environment.

`web/dist` can be built by

    $ npx vite build web

[novnc]: https://github.com/novnc/noVNC
