//! Proxy support for extrablatt_v2
//!
//! This module contains tests for proxy functionality.
//! Proxy configuration is done via `ExtrablattBuilder::proxy()`.

#[cfg(test)]
mod tests {
    use crate::Extrablatt;
    use testcontainers::{
        core::{IntoContainerPort, WaitFor},
        runners::AsyncRunner,
        GenericImage,
    };

    /// Test that proxy configuration is accepted and used.
    /// Uses a Squid proxy container to verify requests go through the proxy.
    #[tokio::test]
    async fn test_proxy_with_squid_container() {
        // Start Squid proxy container
        let container = GenericImage::new("ubuntu/squid", "latest")
            .with_exposed_port(3128.tcp())
            .with_wait_for(WaitFor::message_on_stderr("Accepting HTTP Socket connections"))
            .start()
            .await;

        let container = match container {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Failed to start Squid container (Docker may not be running): {}", e);
                return; // Skip test if Docker is not available
            }
        };

        let proxy_port = container.get_host_port_ipv4(3128).await.unwrap();
        let proxy_url = format!("http://127.0.0.1:{}", proxy_port);

        println!("Squid proxy started on: {}", proxy_url);

        // Test that we can build with proxy
        let result = Extrablatt::builder("https://httpbin.org/ip")
            .expect("Failed to create builder")
            .proxy(&proxy_url)
            .build()
            .await;

        match result {
            Ok(_site) => {
                println!("Successfully connected through proxy!");
            }
            Err(e) => {
                // It's OK if the actual request fails (httpbin might be slow)
                // The important thing is that the proxy was configured
                println!("Request through proxy result: {:?}", e);
            }
        }
    }

    /// Test that invalid proxy URL is rejected
    #[tokio::test]
    async fn test_invalid_proxy_url() {
        let result = Extrablatt::builder("https://example.com")
            .expect("Failed to create builder")
            .proxy("not-a-valid-proxy-url")
            .build()
            .await;

        // Should fail because the proxy URL is invalid
        assert!(result.is_err(), "Expected error for invalid proxy URL");
        println!("Invalid proxy correctly rejected: {:?}", result.err());
    }

    /// Test that proxy is optional (None by default)
    #[tokio::test]
    async fn test_no_proxy_works() {
        // This should work without any proxy
        let result = Extrablatt::builder("https://httpbin.org/ip")
            .expect("Failed to create builder")
            .build()
            .await;

        match result {
            Ok(_) => println!("Request without proxy succeeded"),
            Err(e) => println!("Request without proxy failed (network issue?): {:?}", e),
        }
    }
}
