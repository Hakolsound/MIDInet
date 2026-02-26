/// macOS CoreMIDI virtual device implementation.
/// Creates a virtual MIDI source and destination that appears to apps
/// (Resolume Arena, Ableton, etc.) as if the physical controller is connected.

use std::sync::{Arc, Mutex};

use coremidi::{Client, Destinations, PacketBuffer, Sources, VirtualDestination, VirtualSource};
use tracing::{debug, info};

use crate::virtual_device::VirtualMidiDevice;
use midi_protocol::identity::DeviceIdentity;

pub struct CoreMidiVirtualDevice {
    name: String,
    client: Option<Client>,
    /// Virtual source: we push MIDI data here → apps receive it as input
    source: Option<VirtualSource>,
    /// Virtual destination: apps send MIDI here → we receive it for bidirectional feedback
    destination: Option<VirtualDestination>,
    /// Buffer for received MIDI data from apps (feedback path)
    feedback_buffer: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl CoreMidiVirtualDevice {
    pub fn new() -> Self {
        Self {
            name: String::new(),
            client: None,
            source: None,
            destination: None,
            feedback_buffer: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl VirtualMidiDevice for CoreMidiVirtualDevice {
    fn create(&mut self, identity: &DeviceIdentity) -> anyhow::Result<()> {
        self.name = identity.name.clone();

        // Create CoreMIDI client
        let client = Client::new(&format!("MIDInet-{}", &self.name)).map_err(|e| {
            anyhow::anyhow!("Failed to create CoreMIDI client: {:?}", e)
        })?;

        // Create virtual source — apps see this as an available MIDI input device.
        // The name must match the physical controller exactly so Resolume
        // uses the same controller script and mappings.
        let source = client.virtual_source(&self.name).map_err(|e| {
            anyhow::anyhow!("Failed to create virtual source '{}': {:?}", self.name, e)
        })?;

        // Create virtual destination — apps send MIDI feedback here (LED control, fader positions).
        // We capture incoming data in the callback and buffer it for the focus/feedback path.
        let feedback_buf = Arc::clone(&self.feedback_buffer);
        let dest_name = self.name.clone();

        let destination = client
            .virtual_destination(&format!("{} Output", &self.name), move |packet_list| {
                // CoreMIDI callback: called on a CoreMIDI thread when apps send MIDI to us
                for packet in packet_list.iter() {
                    let data = packet.data().to_vec();
                    if let Ok(mut buf) = feedback_buf.lock() {
                        // Cap buffer to prevent unbounded growth
                        if buf.len() < 4096 {
                            buf.push(data);
                        } else {
                            // Drop oldest
                            buf.remove(0);
                            buf.push(packet.data().to_vec());
                        }
                    }
                }
            })
            .map_err(|e| {
                anyhow::anyhow!("Failed to create virtual destination '{}': {:?}", dest_name, e)
            })?;

        info!(
            name = %self.name,
            source_count = Sources::count(),
            dest_count = Destinations::count(),
            "CoreMIDI virtual device created"
        );

        self.client = Some(client);
        self.source = Some(source);
        self.destination = Some(destination);

        Ok(())
    }

    fn send(&self, data: &[u8]) -> anyhow::Result<()> {
        let source = self.source.as_ref().ok_or_else(|| {
            anyhow::anyhow!("Virtual source not created")
        })?;

        // Build a CoreMIDI packet with the MIDI data
        let packet_buf = PacketBuffer::new(0, data);
        source.received(&packet_buf).map_err(|e| {
            anyhow::anyhow!("Failed to send MIDI data: {:?}", e)
        })?;

        debug!(bytes = data.len(), "Sent MIDI to virtual source");
        Ok(())
    }

    fn receive(&self) -> anyhow::Result<Option<Vec<u8>>> {
        if let Ok(mut buf) = self.feedback_buffer.lock() {
            if buf.is_empty() {
                Ok(None)
            } else {
                // Return oldest message (FIFO)
                Ok(Some(buf.remove(0)))
            }
        } else {
            Ok(None)
        }
    }

    fn close(&mut self) -> anyhow::Result<()> {
        info!(name = %self.name, "Closing CoreMIDI virtual device");
        // Dropping the source/destination/client handles unregisters them from CoreMIDI
        self.source = None;
        self.destination = None;
        self.client = None;
        Ok(())
    }

    fn device_name(&self) -> &str {
        &self.name
    }
}
