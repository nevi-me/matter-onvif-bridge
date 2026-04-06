//! Camera registry — central store of discovered cameras with broadcast events.
//!
//! Mirrors the TypeScript CameraRegistry with event-based add/remove notifications.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use tokio::sync::broadcast;
use tracing::{debug, info};

use crate::types::CameraDevice;

/// Events emitted by the camera registry.
#[derive(Debug, Clone)]
pub enum RegistryEvent {
    Added(CameraDevice),
    Removed(String),
    Updated(CameraDevice),
}

/// Central store of discovered ONVIF cameras.
///
/// Thread-safe via Arc<RwLock<>> for state and broadcast channels for events.
#[derive(Clone)]
pub struct CameraRegistry {
    cameras: Arc<RwLock<HashMap<String, CameraDevice>>>,
    tx: broadcast::Sender<RegistryEvent>,
}

impl CameraRegistry {
    /// Create a new camera registry with the given broadcast channel capacity.
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self {
            cameras: Arc::new(RwLock::new(HashMap::new())),
            tx,
        }
    }

    /// Subscribe to registry events.
    pub fn subscribe(&self) -> broadcast::Receiver<RegistryEvent> {
        self.tx.subscribe()
    }

    /// Add or update a camera in the registry.
    pub fn add_camera(&self, camera: CameraDevice) {
        let mut cameras = self.cameras.write().unwrap();
        let id = camera.id.clone();

        if cameras.contains_key(&id) {
            info!(camera_id = id, "Camera updated in registry");
            cameras.insert(id, camera.clone());
            let _ = self.tx.send(RegistryEvent::Updated(camera));
        } else {
            info!(
                camera_id = id,
                manufacturer = camera.device_info.manufacturer,
                model = camera.device_info.model,
                "Camera added to registry"
            );
            cameras.insert(id, camera.clone());
            let _ = self.tx.send(RegistryEvent::Added(camera));
        }
    }

    /// Remove a camera from the registry by ID.
    pub fn remove_camera(&self, id: &str) {
        let mut cameras = self.cameras.write().unwrap();
        if cameras.remove(id).is_some() {
            info!(camera_id = id, "Camera removed from registry");
            let _ = self.tx.send(RegistryEvent::Removed(id.to_string()));
        } else {
            debug!(camera_id = id, "Attempted to remove unknown camera");
        }
    }

    /// Get a camera by ID.
    pub fn get(&self, id: &str) -> Option<CameraDevice> {
        let cameras = self.cameras.read().unwrap();
        cameras.get(id).cloned()
    }

    /// Get all cameras.
    pub fn get_all(&self) -> Vec<CameraDevice> {
        let cameras = self.cameras.read().unwrap();
        cameras.values().cloned().collect()
    }

    /// Get the number of cameras in the registry.
    pub fn len(&self) -> usize {
        let cameras = self.cameras.read().unwrap();
        cameras.len()
    }

    /// Check if the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
