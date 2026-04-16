#[cfg(test)]
mod tests {
    use super::super::*;
    use tempfile::TempDir;

    fn test_config(dir: &TempDir) -> config::RegistryConfig {
        config::RegistryConfig {
            enabled: true,
            local_path: dir.path().to_path_buf(),
            remote_url: String::new(),
            auto_sync: false,
            sync_interval_minutes: 15,
            practitioner_id: "william".to_string(),
        }
    }

    fn sample_client(id: &str) -> types::RegistryClient {
        types::RegistryClient {
            client_id: id.to_string(),
            name: format!("Test Client {}", id),
            dob: Some("1990-01-15".to_string()),
            address: Some("123 Test St".to_string()),
            phone: Some("07700900000".to_string()),
            email: Some("test@example.com".to_string()),
            tm3_id: Some(1234),
            status: "active".to_string(),
            discharge_date: None,
            funding: types::RegistryFunding {
                funding_type: Some("self-pay".to_string()),
                rate: Some(220.0),
                session_duration: Some(50),
                ..Default::default()
            },
            referrer: types::RegistryReferrer {
                name: Some("Dr Test".to_string()),
                ..Default::default()
            },
            diagnosis: None,
            diagnostic_code: None,
        }
    }

    #[test]
    fn test_init_repo_creates_structure() {
        let dir = TempDir::new().unwrap();
        repo::init_repo(dir.path()).unwrap();

        assert!(dir.path().join(".git").exists());
        assert!(dir.path().join("clients").exists());
        assert!(dir.path().join("calendars").exists());
        assert!(dir.path().join("attendance").exists());
        assert!(dir.path().join("config").exists());
        assert!(dir.path().join("config/practice.yaml").exists());
        assert!(dir.path().join("config/practitioners.yaml").exists());
    }

    #[test]
    fn test_repo_status_new() {
        let dir = TempDir::new().unwrap();
        repo::init_repo(dir.path()).unwrap();

        let status = repo::status(dir.path()).unwrap();
        assert!(status.is_repo);
        assert!(!status.has_remote);
        assert!(status.clean);
        assert_eq!(status.uncommitted_count, 0);
    }

    #[test]
    fn test_repo_status_not_a_repo() {
        let dir = TempDir::new().unwrap();
        let status = repo::status(dir.path()).unwrap();
        assert!(!status.is_repo);
    }

    #[test]
    fn test_save_and_load_client() {
        let dir = TempDir::new().unwrap();
        repo::init_repo(dir.path()).unwrap();
        let config = test_config(&dir);

        let client = sample_client("TC01");
        client::save_client(&config, &client).unwrap();

        let loaded = client::get_client(&config, "TC01").unwrap();
        assert_eq!(loaded.client_id, "TC01");
        assert_eq!(loaded.name, "Test Client TC01");
        assert_eq!(loaded.dob, Some("1990-01-15".to_string()));
        assert_eq!(loaded.tm3_id, Some(1234));
        assert_eq!(loaded.status, "active");
        assert_eq!(
            loaded.funding.funding_type,
            Some("self-pay".to_string())
        );
        assert_eq!(loaded.funding.rate, Some(220.0));
        assert_eq!(
            loaded.referrer.name,
            Some("Dr Test".to_string())
        );
    }

    #[test]
    fn test_list_clients() {
        let dir = TempDir::new().unwrap();
        repo::init_repo(dir.path()).unwrap();
        let config = test_config(&dir);

        client::save_client(&config, &sample_client("AB01")).unwrap();
        client::save_client(&config, &sample_client("CD02")).unwrap();
        client::save_client(&config, &sample_client("EF03")).unwrap();

        let ids = client::list_client_ids(&config).unwrap();
        assert_eq!(ids, vec!["AB01", "CD02", "EF03"]);

        let clients = client::list_clients(&config).unwrap();
        assert_eq!(clients.len(), 3);
        assert_eq!(clients[0].name, "Test Client AB01");
    }

    #[test]
    fn test_list_empty_registry() {
        let dir = TempDir::new().unwrap();
        repo::init_repo(dir.path()).unwrap();
        let config = test_config(&dir);

        let ids = client::list_client_ids(&config).unwrap();
        assert!(ids.is_empty());
    }

    #[test]
    fn test_get_nonexistent_client() {
        let dir = TempDir::new().unwrap();
        repo::init_repo(dir.path()).unwrap();
        let config = test_config(&dir);

        let result = client::get_client(&config, "NOPE");
        assert!(result.is_err());
    }

    #[test]
    fn test_delete_client() {
        let dir = TempDir::new().unwrap();
        repo::init_repo(dir.path()).unwrap();
        let config = test_config(&dir);

        client::save_client(&config, &sample_client("DEL1")).unwrap();
        assert!(config.client_dir("DEL1").exists());

        client::delete_client(&config, "DEL1").unwrap();
        assert!(!config.client_dir("DEL1").exists());
    }

    #[test]
    fn test_assignments() {
        let dir = TempDir::new().unwrap();
        repo::init_repo(dir.path()).unwrap();
        let config = test_config(&dir);

        client::save_client(&config, &sample_client("AS01")).unwrap();

        let assignments = vec![
            types::PractitionerAssignment {
                practitioner_id: "william".to_string(),
                since: "2026-01-01".to_string(),
                primary: true,
            },
            types::PractitionerAssignment {
                practitioner_id: "colleague-a".to_string(),
                since: "2026-03-01".to_string(),
                primary: false,
            },
        ];

        client::save_assignments(&config, "AS01", &assignments).unwrap();

        let loaded = client::get_assignments(&config, "AS01").unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].practitioner_id, "william");
        assert!(loaded[0].primary);
        assert_eq!(loaded[1].practitioner_id, "colleague-a");
        assert!(!loaded[1].primary);
    }

    #[test]
    fn test_count_by_status() {
        let dir = TempDir::new().unwrap();
        repo::init_repo(dir.path()).unwrap();
        let config = test_config(&dir);

        let mut active = sample_client("AC01");
        active.status = "active".to_string();
        client::save_client(&config, &active).unwrap();

        let mut discharged = sample_client("DC01");
        discharged.status = "discharged".to_string();
        client::save_client(&config, &discharged).unwrap();

        let (a, d) = client::count_by_status(&config).unwrap();
        assert_eq!(a, 1);
        assert_eq!(d, 1);
    }

    #[test]
    fn test_add_and_commit() {
        let dir = TempDir::new().unwrap();
        repo::init_repo(dir.path()).unwrap();
        let config = test_config(&dir);

        client::save_client(&config, &sample_client("CM01")).unwrap();

        repo::add_and_commit(
            dir.path(),
            &["clients/CM01/"],
            "Test commit",
        )
        .unwrap();

        let status = repo::status(dir.path()).unwrap();
        assert!(status.clean);
    }

    #[test]
    fn test_add_and_commit_noop_when_clean() {
        let dir = TempDir::new().unwrap();
        repo::init_repo(dir.path()).unwrap();

        // Committing when clean should be a no-op, not an error
        repo::add_and_commit(dir.path(), &["."], "Nothing to commit").unwrap();
    }

    #[test]
    fn test_has_no_remote() {
        let dir = TempDir::new().unwrap();
        repo::init_repo(dir.path()).unwrap();

        assert!(!repo::has_remote(dir.path()).unwrap());
    }

    #[test]
    fn test_sync_due_no_marker() {
        let dir = TempDir::new().unwrap();
        repo::init_repo(dir.path()).unwrap();
        let config = test_config(&dir);

        assert!(sync::sync_due(&config));
    }

    #[test]
    fn test_sync_marker() {
        let dir = TempDir::new().unwrap();
        repo::init_repo(dir.path()).unwrap();
        let config = test_config(&dir);

        sync::mark_synced(&config).unwrap();
        assert!(!sync::sync_due(&config));
    }

    #[test]
    fn test_import_from_clinical() {
        let dir = TempDir::new().unwrap();
        repo::init_repo(dir.path()).unwrap();
        let config = test_config(&dir);

        // Create a fake clinical directory with a client
        let clinical_root = TempDir::new().unwrap();
        let client_dir = clinical_root.path().join("clients").join("IM01");
        std::fs::create_dir_all(&client_dir).unwrap();

        let identity_yaml = r#"---
name: "Import, Test"
dob: "1985-06-20"
phone: "07700111222"
email: "import@test.com"
tm3_id: 5678
status: active
funding:
  type: "BUPA"
  rate: 200
  session_duration: 50
referrer:
  name: "Dr Importer"
  role: "GP"
"#;
        std::fs::write(client_dir.join("identity.yaml"), identity_yaml).unwrap();

        // Also create a correspondence file
        let corr_dir = client_dir.join("correspondence");
        std::fs::create_dir_all(&corr_dir).unwrap();
        std::fs::write(corr_dir.join("referral.md"), "Referral letter content").unwrap();

        // Import
        let clinical_path = clinical_root.path().to_path_buf();
        import::import_client(&config, "IM01", &clinical_path).unwrap();

        // Verify imported
        let loaded = client::get_client(&config, "IM01").unwrap();
        assert_eq!(loaded.name, "Import, Test");
        assert_eq!(loaded.dob, Some("1985-06-20".to_string()));
        assert_eq!(loaded.tm3_id, Some(5678));
        assert_eq!(loaded.funding.funding_type, Some("BUPA".to_string()));
        assert_eq!(loaded.funding.rate, Some(200.0));
        assert_eq!(loaded.referrer.name, Some("Dr Importer".to_string()));

        // Verify correspondence copied
        assert!(config
            .client_dir("IM01")
            .join("correspondence")
            .join("referral.md")
            .exists());

        // Verify assignment created
        let assignments = client::get_assignments(&config, "IM01").unwrap();
        assert_eq!(assignments.len(), 1);
        assert_eq!(assignments[0].practitioner_id, "william");
        assert!(assignments[0].primary);
    }

    #[test]
    fn test_import_all_skips_existing() {
        let dir = TempDir::new().unwrap();
        repo::init_repo(dir.path()).unwrap();
        let config = test_config(&dir);

        // Create fake clinical directory with two clients
        let clinical_root = TempDir::new().unwrap();
        for id in &["SK01", "SK02"] {
            let client_dir = clinical_root.path().join("clients").join(id);
            std::fs::create_dir_all(&client_dir).unwrap();
            std::fs::write(
                client_dir.join("identity.yaml"),
                format!("---\nname: \"Client {}\"\nstatus: active\n", id),
            )
            .unwrap();
        }

        // Import SK01 first
        let clinical_path = clinical_root.path().to_path_buf();
        import::import_client(&config, "SK01", &clinical_path).unwrap();

        // Import all — SK01 should be skipped
        let (imported, skipped, errors) =
            import::import_all(&config, &clinical_path).unwrap();
        assert_eq!(imported, 1); // SK02
        assert_eq!(skipped, 1); // SK01
        assert_eq!(errors, 0);
    }

    #[test]
    fn test_registry_config_defaults() {
        let config = config::RegistryConfig::default();
        assert!(!config.enabled);
        assert!(config.local_path.ends_with("Clinical/registry"));
        assert!(config.remote_url.is_empty());
        assert!(config.auto_sync);
        assert_eq!(config.sync_interval_minutes, 15);
    }

    #[test]
    fn test_client_dir_paths() {
        let dir = TempDir::new().unwrap();
        let config = test_config(&dir);

        assert_eq!(
            config.clients_dir(),
            dir.path().join("clients")
        );
        assert_eq!(
            config.client_dir("EB76"),
            dir.path().join("clients").join("EB76")
        );
        assert_eq!(
            config.config_dir(),
            dir.path().join("config")
        );
    }

    #[test]
    fn test_format_client() {
        let client = sample_client("FM01");
        let formatted = client::format_client(&client);
        assert!(formatted.contains("FM01"));
        assert!(formatted.contains("Test Client FM01"));
        assert!(formatted.contains("07700900000"));
        assert!(formatted.contains("1234")); // TM3 ID
        assert!(formatted.contains("self-pay"));
        assert!(formatted.contains("Dr Test"));
    }

    #[test]
    fn test_save_creates_subdirs() {
        let dir = TempDir::new().unwrap();
        repo::init_repo(dir.path()).unwrap();
        let config = test_config(&dir);

        client::save_client(&config, &sample_client("SD01")).unwrap();

        assert!(config.client_dir("SD01").join("letters").exists());
        assert!(config.client_dir("SD01").join("correspondence").exists());
    }

    #[test]
    fn test_identity_yaml_roundtrip() {
        // Verify that saving and loading preserves the YAML structure
        let dir = TempDir::new().unwrap();
        repo::init_repo(dir.path()).unwrap();
        let config = test_config(&dir);

        let original = types::RegistryClient {
            client_id: "RT01".to_string(),
            name: "Roundtrip, Test".to_string(),
            dob: Some("1975-12-25".to_string()),
            address: None, // Explicitly None — should not appear in YAML
            phone: Some("07700999888".to_string()),
            email: None,
            tm3_id: Some(9999),
            status: "active".to_string(),
            discharge_date: None,
            funding: types::RegistryFunding {
                funding_type: Some("AXA".to_string()),
                rate: Some(250.0),
                session_duration: Some(45),
                contact: None,
                policy: Some("POL-123".to_string()),
                email: Some("claims@axa.com".to_string()),
            },
            referrer: types::RegistryReferrer {
                name: Some("Dr Roundtrip".to_string()),
                role: Some("Consultant Psychiatrist".to_string()),
                practice: Some("Test Clinic".to_string()),
                email: Some("dr@test.com".to_string()),
                credentials: Some("MB BS MRCPsych".to_string()),
                gmc: Some("1234567".to_string()),
            },
            diagnosis: Some("Adjustment disorder".to_string()),
            diagnostic_code: Some("F43.2".to_string()),
        };

        client::save_client(&config, &original).unwrap();
        let loaded = client::get_client(&config, "RT01").unwrap();

        assert_eq!(loaded.name, original.name);
        assert_eq!(loaded.dob, original.dob);
        assert_eq!(loaded.address, original.address);
        assert_eq!(loaded.phone, original.phone);
        assert_eq!(loaded.tm3_id, original.tm3_id);
        assert_eq!(loaded.funding.funding_type, original.funding.funding_type);
        assert_eq!(loaded.funding.rate, original.funding.rate);
        assert_eq!(loaded.funding.policy, original.funding.policy);
        assert_eq!(loaded.referrer.name, original.referrer.name);
        assert_eq!(loaded.referrer.gmc, original.referrer.gmc);
        assert_eq!(loaded.diagnosis, original.diagnosis);
        assert_eq!(loaded.diagnostic_code, original.diagnostic_code);
    }
}
