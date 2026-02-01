use std::time::Duration;

use async_channel::{bounded, Receiver, Sender};
use reqwest::Client;
use tracing::{error, info, warn};

use crate::config::WebhookConfig;
use crate::domain::TransactionEvent;

pub struct WebhookDispatcher {
    sender: Sender<TransactionEvent>,
    config: WebhookConfig,
}

impl WebhookDispatcher {
    pub fn new(config: WebhookConfig) -> Self {
        let (sender, receiver) = bounded(config.queue_size);

        let dispatcher = Self {
            sender,
            config: config.clone(),
        };

        if config.enabled {
            tokio::spawn(Self::worker(receiver, config));
        }

        dispatcher
    }

    pub async fn dispatch(&self, event: TransactionEvent) {
        if !self.config.enabled {
            return;
        }

        if let Err(e) = self.sender.try_send(event) {
            warn!(error = %e, "Failed to enqueue webhook event (queue full)");
        }
    }

    async fn worker(receiver: Receiver<TransactionEvent>, config: WebhookConfig) {
        let client = Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .build()
            .expect("Failed to create HTTP client");

        let endpoint = match &config.endpoint_url {
            Some(url) => url.clone(),
            None => {
                warn!("Webhook enabled but no endpoint URL configured");
                return;
            }
        };

        info!(endpoint = %endpoint, "Webhook dispatcher started");

        while let Ok(event) = receiver.recv().await {
            let event_type = event.event_type.as_str();
            let transaction_id = event.transaction_id.to_string();

            let mut retries = 0;
            let mut delay = Duration::from_millis(100);

            loop {
                let result = client
                    .post(&endpoint)
                    .header("Content-Type", "application/json")
                    .header("X-Event-Type", event_type)
                    .header("X-Transaction-Id", &transaction_id)
                    .header("X-Idempotency-Key", &event.idempotency_key)
                    .json(&event)
                    .send()
                    .await;

                match result {
                    Ok(response) if response.status().is_success() => {
                        info!(
                            event_type = %event_type,
                            transaction_id = %transaction_id,
                            "Webhook delivered successfully"
                        );
                        break;
                    }
                    Ok(response) => {
                        warn!(
                            event_type = %event_type,
                            transaction_id = %transaction_id,
                            status = %response.status(),
                            "Webhook delivery failed with status"
                        );
                    }
                    Err(e) => {
                        warn!(
                            event_type = %event_type,
                            transaction_id = %transaction_id,
                            error = %e,
                            "Webhook delivery error"
                        );
                    }
                }

                retries += 1;
                if retries >= config.max_retries {
                    error!(
                        event_type = %event_type,
                        transaction_id = %transaction_id,
                        "Webhook delivery failed after {} retries",
                        config.max_retries
                    );
                    break;
                }

                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(Duration::from_secs(30));
            }
        }

        info!("Webhook dispatcher stopped");
    }

    pub fn queue_len(&self) -> usize {
        self.sender.len()
    }

    pub fn is_full(&self) -> bool {
        self.sender.is_full()
    }
}

impl Clone for WebhookDispatcher {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            config: self.config.clone(),
        }
    }
}
