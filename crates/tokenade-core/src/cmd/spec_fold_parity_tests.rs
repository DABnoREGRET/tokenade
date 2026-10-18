// The tests of `spec_fold_parity`.
//
// Spliced by `include!` from `spec_fold_parity.rs`: the same module, so the same
// private access and the same `super::` paths, without a single visibility
// line changed. It is the shape this repo already uses for 33 other modules.
//
// A `//!` is impossible here: a file spliced by `include!` is not a
// module, and an INNER doc comment has nothing to qualify.

#[cfg(test)]
mod tests {
    use crate::cmd::spec::{apply, Detect, Spec};
    use crate::cmd::spec_table_folds::FOLD_SPECS;

    /// The needles each row was GENERATED from, frozen here.
    ///
    /// Deliberately not re-derived from `spec.detect`: `every_needle(&spec.detect)`
    /// built the probe list out of the very thing under test, so NARROWING a gate
    /// also deleted the probe that would have caught it. MEASURED: dropping
    /// `"DATE"` from `argocd_app_history` passed a battery that derived its own
    /// inputs, and fails this one.
    static STUB_NEEDLES: &[(&str, &[&str])] = &[
        ("accesschk", &[]),
        ("addusers", &[]),
        ("airport_scan", &["SSID", "BSSID", ":"]),
        ("airport_scan_full", &["SSID", "BSSID"]),
        ("appcmd_list_app", &["APP", "applicationPool"]),
        ("appcmd_list_apppool", &["APPPOOL", "MgdVersion"]),
        ("appcmd_list_site", &["SITE", "bindings", "state"]),
        ("appcmd_list_vdir", &["VDIR", "physicalPath"]),
        ("argocd_app_history", &["ID", "DATE"]),
        ("argocd_app_list", &["NAME", "CLUSTER", "NAMESPACE"]),
        ("argocd_cluster_list", &["SERVER", "VERSION"]),
        ("argocd_repo_list", &["TYPE", "REPO"]),
        ("arp", &["Interface", "Internet Address"]),
        ("arp_win", &["Interface", "Internet Address"]),
        ("at", &[]),
        ("auditpol", &["Category/Subcategory", "Setting"]),
        ("auditpol_get_category", &["Category", "Setting"]),
        ("autorunsc", &["Publisher", "ImagePath"]),
        ("aws_logs_tail", &[]),
        ("az_webapp_log_tail", &[]),
        ("brew_services", &["Name", "Status"]),
        ("btrfs_device_stats", &["[", "]", "0"]),
        ("btrfs_filesystem_df", &["/"]),
        ("btrfs_qgroup_show", &["qgroupid", "referenced"]),
        ("bzip2", &[":", "."]),
        ("cal", &[]),
        (
            "cargo_install_log",
            &["Compiling", "Installing", "Finished"],
        ),
        ("cargo_llvm_cov", &["Filename", "Regions", "Lines"]),
        ("cargo_nextest", &["PASS", "FAIL", "test["]),
        ("cargo_outdated", &["Name", "Compat", "Latest"]),
        ("cargo_test_no_run", &["Finished", "Executable"]),
        ("cargo_test_nocapture", &[]),
        ("cargo_test_quiet", &["test ", "FAILED", "passed"]),
        ("chmod", &[]),
        ("cp", &[]),
        ("crmod", &[]),
        ("curl", &[]),
        (
            "curl_progress",
            &["% Total", "Dload  Upload", "Average Speed"],
        ),
        ("curl_win", &[]),
        ("date", &[]),
        ("date_cmd", &[]),
        ("df_mac_h", &["Filesystem", "Size"]),
        ("dnf_repolist", &["repo id", "Repo-id"]),
        ("docker_attach", &["Attaching", "CONTAINER"]),
        ("docker_compose", &["SERVICE", "STATUS", "COMMAND"]),
        ("docker_compose_logs", &[]),
        ("docker_context_ls", &["DOCKER ENDPOINT"]),
        ("docker_cp", &["Successfully copied", ":", "/", "CONTAINER"]),
        ("docker_history", &["CREATED BY"]),
        ("docker_images", &["REPOSITORY", "TAG", "IMAGE ID"]),
        ("docker_logs_follow", &[]),
        ("docker_logs_tail", &[]),
        ("docker_network", &["NETWORK ID", "DRIVER", "SCOPE"]),
        ("docker_node_ls", &["HOSTNAME", "AVAILABILITY"]),
        ("docker_plugin_ls", &["ENABLED", "NAME"]),
        ("docker_ps", &["CONTAINER ID", "STATUS", "PORTS"]),
        ("docker_search", &["STARS", "DESCRIPTION"]),
        ("docker_secret_ls", &["ID", "CREATED", "UPDATED"]),
        ("docker_service_ls", &["REPLICAS", "MODE", "IMAGE"]),
        ("docker_service_ps", &["DESIRED STATE", "CURRENT STATE"]),
        ("docker_stats", &["CPU %", "MEM USAGE"]),
        ("docker_top_proc", &["UID", "PID", "CMD"]),
        ("docker_volume", &["DRIVER", "VOLUME NAME"]),
        ("dpkg_l", &["ii  ", "rc  ", "un  "]),
        ("dscl_list_groups", &[".", "  "]),
        ("dsmod", &[]),
        ("dstat", &["total-cpu", "recv", "send"]),
        ("expand_cmd", &[]),
        ("fc_cmd", &[]),
        ("find_delete", &["/"]),
        ("find_print0", &["/"]),
        ("flux_get_helmreleases", &["NAME", "REVISION"]),
        ("flux_get_kustomizations", &["NAME", "REVISION"]),
        ("free_s", &["total", "used", "free"]),
        ("gcloud_auth_list", &["ACTIVE", "ACCOUNT"]),
        ("gcloud_compute_addresses_list", &["ADDRESS/RANGE"]),
        ("gcloud_compute_disks_list", &["SIZE_GB"]),
        (
            "gcloud_compute_firewall_rules_list",
            &["DIRECTION", "PRIORITY"],
        ),
        ("gcloud_compute_forwarding_rules_list", &["IP_PROTOCOL"]),
        ("gcloud_compute_images_list", &["DEPRECATED"]),
        ("gcloud_compute_instances", &["MACHINE_TYPE", "INTERNAL_IP"]),
        (
            "gcloud_compute_networks_list",
            &["SUBNET_MODE", "BGP_ROUTING_MODE"],
        ),
        (
            "gcloud_compute_networks_subnets_list",
            &["REGION", "NETWORK", "RANGE"],
        ),
        ("gcloud_compute_snapshots_list", &["SRC_DISK"]),
        (
            "gcloud_compute_target_pools_list",
            &["SESSION_AFFINITY", "HEALTH_CHECKS"],
        ),
        (
            "gcloud_config_configurations_list",
            &["IS_ACTIVE", "ACCOUNT"],
        ),
        (
            "gcloud_container_node_pools_list",
            &["MACHINE_TYPE", "NODE_VERSION"],
        ),
        ("gcloud_functions_list", &["STATE", "TRIGGER"]),
        ("gcloud_functions_logs_read", &[":"]),
        ("gcloud_iam_roles_list", &["STAGE"]),
        (
            "gcloud_iam_service_accounts_keys_list",
            &["KEY_ID", "CREATED_AT"],
        ),
        (
            "gcloud_iam_service_accounts_list",
            &["EMAIL", "DISPLAY_NAME"],
        ),
        (
            "gcloud_kubernetes_clusters",
            &["MASTER_VERSION", "NUM_NODES"],
        ),
        ("gcloud_logging_read", &[":"]),
        ("gcloud_projects_list", &["PROJECT_ID", "PROJECT_NUMBER"]),
        ("gcloud_run_revisions_list", &["SERVING_PERCENT"]),
        (
            "gcloud_run_services_list",
            &["LAST DEPLOYED BY", "LAST DEPLOYED AT"],
        ),
        ("gcloud_secrets_list", &["REPLICATION_POLICY"]),
        (
            "gcloud_secrets_versions_list",
            &["DESTROY_AT", "CREATED_AT", "STATE"],
        ),
        ("gcloud_services_list", &["TITLE", "STATE"]),
        ("gcloud_sql_databases_list", &["CHARSET", "COLLATION"]),
        ("gcloud_sql_instances_list", &["DATABASE_VERSION", "TIER"]),
        ("gcloud_sql_users_list", &["HOST", "TYPE"]),
        ("get_eventlog", &["Index", "EntryType", "Source"]),
        ("get_process", &["Handles", "ProcessName", "NPM"]),
        ("get_service", &["Status", "DisplayName", "Name"]),
        ("getfileinfo", &[":"]),
        ("gh_alias_list", &["alias", ":", "="]),
        (
            "gh_codespace_create",
            &["codespace", "codespace", "github", "Creating", "NAME"],
        ),
        (
            "gh_codespace_delete",
            &["codespace", "codespace", "github", "Creating", "NAME"],
        ),
        (
            "gh_codespace_list",
            &["DISPLAY NAME", "codespace", "github", "Creating"],
        ),
        (
            "gh_codespace_ssh",
            &["codespace", "codespace", "github", "Creating", "NAME"],
        ),
        (
            "gh_gist_list",
            &["gist", "gist.github.com", "https://gist", "ID"],
        ),
        ("gh_issue_list", &["TITLE", "#"]),
        ("gh_pr_list", &["TITLE", "#"]),
        (
            "gh_pr_reopen",
            &[
                "github.com",
                "pull request",
                "pull request",
                "github.com",
                "#",
                "PR",
                "NUMBER",
            ],
        ),
        (
            "gh_release_delete",
            &[
                "github.com",
                "release",
                "release",
                "github.com",
                "tag",
                "TAG",
            ],
        ),
        ("gh_release_list", &[]),
        (
            "gh_release_upload",
            &[
                "github.com",
                "release",
                "release",
                "github.com",
                "tag",
                "TAG",
            ],
        ),
        (
            "gh_repo_create",
            &[
                "github.com",
                "repo",
                "repo",
                "github.com",
                "repository",
                "NAME",
            ],
        ),
        ("gh_repo_list", &["CLONE", "/"]),
        (
            "gh_run_cancel",
            &[
                "cancelled",
                "Run",
                "workflow",
                "run",
                "github.com",
                "completed",
                "ID",
            ],
        ),
        ("gh_run_list", &["STATUS", "WORKFLOW"]),
        (
            "gh_run_rerun",
            &[
                "rerun",
                "Run",
                "workflow",
                "run",
                "github.com",
                "completed",
                "ID",
            ],
        ),
        (
            "gh_run_watch",
            &[
                "Run",
                "STATUS",
                "workflow",
                "run",
                "github.com",
                "completed",
            ],
        ),
        (
            "gh_secret_list",
            &["secret", "github.com", "Updated", "NAME"],
        ),
        (
            "gh_secret_set",
            &["secret", "github.com", "Updated", "NAME"],
        ),
        ("git_branch_all", &["remotes/", "* "]),
        ("git_branch_remote", &["remotes/", "origin/", "upstream/"]),
        ("git_diff_name_only", &["/", "."]),
        ("git_lfs_ls_files", &["*", "/"]),
        ("git_ls_tree", &["blob ", "tree "]),
        ("git_reflog", &["HEAD@{", "ORIG_HEAD"]),
        ("git_revert", &["Revert", "[", "commit"]),
        ("git_show", &["commit ", "diff --git", "Author:"]),
        ("git_show_summary", &[":"]),
        ("git_stash_list", &["stash@"]),
        ("git_submodule_foreach", &["Entering '", "submodule"]),
        (
            "go_test",
            &["=== RUN ", "--- PASS", "--- FAIL", "PASS", "FAIL"],
        ),
        ("grep", &[":"]),
        ("grep_context_fold", &[":"]),
        ("grep_count_fold", &[":"]),
        ("grep_l", &["/", "."]),
        ("gzip", &[":", "."]),
        ("helm_dependency_list", &["NAME", "REPOSITORY"]),
        ("helm_history", &["REVISION", "STATUS"]),
        ("helm_list", &["NAME", "NAMESPACE", "REVISION"]),
        ("helm_repo_add", &["has been added", "NAME"]),
        ("helm_repo_list", &["NAME", "URL"]),
        ("helm_repo_remove", &["has been removed", "NAME"]),
        ("helm_rollback", &["Rollback", "NAME"]),
        ("helm_search_hub", &["URL", "CHART VERSION"]),
        ("helm_search_repo", &["CHART VERSION", "APP VERSION"]),
        ("helm_test", &["PASSED", "FAILED", "NAME"]),
        ("helm_uninstall", &["uninstalled", "NAME"]),
        ("hostname_win", &[]),
        ("hwinfo", &[]),
        ("hwinfo_linux", &["Hardware Class:", "model:"]),
        ("id_cmd", &[]),
        ("id_macos", &[]),
        ("info", &[":"]),
        ("info_cli", &[":"]),
        ("ioreg", &["+", "|"]),
        ("ioreg_l", &["+", "|"]),
        ("ioreg_p_IOBluetooth", &["Bluetooth", "+"]),
        ("ioreg_p_IOSCSI", &["SCSI", "+"]),
        ("ioreg_p_IOUSB", &["USB", "+"]),
        ("ioreg_short", &["+", "|"]),
        ("ip_link", &["mtu", "state"]),
        ("ip_link_show", &["mtu", "state"]),
        ("ip_neighbor", &["lladdr", "REACHABLE", "STALE"]),
        ("ip_route", &["via", "dev", "proto"]),
        ("ip_route_show", &["via", "dev", "proto"]),
        ("ip_rule", &["lookup", "from"]),
        ("ip_tunnel", &["gre", "tun", "mode"]),
        ("ipconfig", &["IP Configuration", "IPv4 Address"]),
        ("iptables_L", &["Chain", "target"]),
        ("iptables_S", &["-P ", "-A "]),
        ("iptables_t_nat", &["Chain", "MASQUERADE"]),
        ("istioctl_proxy_status", &["NAME", "CDS"]),
        ("java", &[]),
        ("journalctl", &["Jan ", "Feb ", "Mar "]),
        ("journalctl_no_pager", &[]),
        ("kextstat", &["Index", "Address"]),
        ("klist_win", &[]),
        ("kubectl_annotate", &["annotated", "NAME"]),
        ("kubectl_api_resources", &["APIVERSION", "KIND"]),
        ("kubectl_auth_reconcile", &["reconciled", "NAME"]),
        ("kubectl_autoscale", &["autoscaled", "NAME"]),
        ("kubectl_certificate_approve", &["approved", "NAME"]),
        ("kubectl_certificate_deny", &["denied", "NAME"]),
        ("kubectl_cordon", &["cordoned", "NAME"]),
        ("kubectl_events", &["LAST SEEN", "REASON"]),
        ("kubectl_exec", &[]),
        ("kubectl_get_clusterrole", &["CREATED AT"]),
        ("kubectl_get_clusterrolebinding", &["ROLE", "AGE"]),
        ("kubectl_get_crd", &["CREATED AT", "SCOPE"]),
        ("kubectl_get_cronjobs", &["SCHEDULE", "LAST SCHEDULE"]),
        ("kubectl_get_csr", &["SIGNERNAME", "REQUESTOR"]),
        ("kubectl_get_daemonset", &["NODE SELECTOR"]),
        ("kubectl_get_deploy", &["UP-TO-DATE", "AVAILABLE", "READY"]),
        ("kubectl_get_endpoints", &["ENDPOINTS", "AGE"]),
        ("kubectl_get_hpa", &["TARGETS", "MINPODS"]),
        ("kubectl_get_ingress", &["HOSTS", "ADDRESS"]),
        ("kubectl_get_jobs", &["COMPLETIONS", "DURATION"]),
        ("kubectl_get_networkpolicy", &["POD-SELECTOR"]),
        ("kubectl_get_nodes", &["ROLES", "VERSION"]),
        ("kubectl_get_ns", &["STATUS", "Active", "Terminating"]),
        ("kubectl_get_pdb", &["MIN AVAILABLE", "DISRUPTIONS"]),
        ("kubectl_get_pods", &["READY", "STATUS", "RESTARTS"]),
        ("kubectl_get_pv", &["RECLAIM POLICY", "CLAIM"]),
        ("kubectl_get_pvc", &["ACCESS MODES", "STORAGECLASS"]),
        ("kubectl_get_replicaset", &["DESIRED", "CURRENT", "READY"]),
        ("kubectl_get_rolebinding", &["ROLE", "AGE"]),
        ("kubectl_get_sa", &["SECRETS", "AGE"]),
        ("kubectl_get_storageclass", &["PROVISIONER"]),
        ("kubectl_get_svc", &["CLUSTER-IP", "EXTERNAL-IP"]),
        ("kubectl_label", &["labeled", "NAME"]),
        ("kubectl_logs", &[]),
        ("kubectl_logs_all", &[]),
        ("kubectl_logs_previous", &[]),
        ("kubectl_proxy", &["Starting to serve", "NAME"]),
        ("kubectl_rollout_undo", &["rolled back", "REVISION"]),
        ("kubectl_scale", &["scaled", "NAME"]),
        ("kubectl_set_env", &["env updated", "NAME"]),
        ("kubectl_set_image", &["image updated", "NAME"]),
        ("kubectl_set_resources", &["resource requirements", "NAME"]),
        ("kubectl_taint", &["tainted", "NAME"]),
        ("kubectl_top", &["CPU(cores)", "MEMORY(bytes)"]),
        ("kubectl_uncordon", &["uncordoned", "NAME"]),
        ("less", &[]),
        ("less_cmd", &[]),
        ("linkerd_cli", &[]),
        ("linkerd_stat", &["NAME", "MESHED"]),
        ("locate", &["/"]),
        ("locate_mac", &["/"]),
        ("loginctl", &["SESSION", "SEAT"]),
        ("loginctl_macos", &["SESSION", "USER"]),
        ("logman_query", &[]),
        ("logout", &["logout", "session", "Saving"]),
        ("logout_macos", &["logout", "session", "Saving"]),
        ("logwatch", &["LogWatch", "Begin", "-"]),
        ("losetup", &["NAME", "BACK-FILE"]),
        ("ls_R", &[]),
        ("machinectl", &["MACHINE", "CLASS"]),
        ("mas_cli", &[]),
        ("mdfind_attr", &["/"]),
        ("mdfind_interpret", &["/"]),
        ("mdfind_live_q", &["/"]),
        ("mdfind_name", &["/"]),
        ("mdfind_onlyin", &["/"]),
        ("mount_t_type", &[" on ", "type "]),
        ("mpstat", &["CPU", "%usr"]),
        ("nc", &[]),
        ("net_macos", &[":", "."]),
        ("netstat_mac_an", &["Proto", "Local Address"]),
        ("netstat_mac_nr", &["Destination", "Gateway"]),
        ("nettop", &["Bytes", "Pkts"]),
        ("nettop_summary", &["Bytes", "Pkts"]),
        ("notify_send", &[]),
        ("npm_outdated", &["Package", "Current", "Wanted"]),
        ("nslookup_win", &["Name:", "Address:"]),
        ("ntfs_util", &[]),
        ("nvidia_smi", &["NVIDIA-SMI", "Driver Version"]),
        ("nvidia_smi_dmon", &["gpu", "sm", "mem"]),
        ("nvidia_smi_pmon", &["gpu", "pid"]),
        ("nvidia_smi_query", &["name", "memory.total"]),
        ("nvidia_smi_topo", &["GPU", "NIC"]),
        ("php", &[]),
        ("pidstat", &["PID", "%usr"]),
        ("pip_list", &["Package", "Version"]),
        ("pip_search", &["LATEST", "DESCRIPTION"]),
        ("pipewire", &[":", "."]),
        ("pipewire_cli", &[":", "."]),
        ("pkgutil_dirs", &["/"]),
        ("pkgutil_files", &["/"]),
        ("pkgutil_only_files", &["/"]),
        ("pmset_schedule", &["scheduled", "Type"]),
        ("popd_cmd", &[]),
        ("powermetrics", &["CPU", "power", "usage"]),
        ("powermetrics_summary", &["CPU", "power"]),
        (
            "powershell_find_module",
            &["Repository", "Description", "Name"],
        ),
        ("powershell_get_acl_ext", &["Owner", "Access"]),
        (
            "powershell_get_adcomputer",
            &["DNSHostName", "DistinguishedName"],
        ),
        (
            "powershell_get_addomain",
            &["InfrastructureMaster", "PDCEmulator"],
        ),
        (
            "powershell_get_addomaincontroller",
            &["OperationMasterRoles", "HostName"],
        ),
        (
            "powershell_get_adforest",
            &["DomainNamingMaster", "Domains"],
        ),
        (
            "powershell_get_adfsaccesspolicy",
            &["AccessControlPolicyName", "RelyingPartyTrustName"],
        ),
        ("powershell_get_adgroup", &["GroupScope", "GroupCategory"]),
        (
            "powershell_get_adgroupmember",
            &["objectClass", "SamAccountName"],
        ),
        (
            "powershell_get_adobject",
            &["DistinguishedName", "ObjectClass"],
        ),
        (
            "powershell_get_aduser",
            &["SamAccountName", "DistinguishedName"],
        ),
        (
            "powershell_get_authenticodesignature",
            &["SignerCertificate", "Path", "Status"],
        ),
        (
            "powershell_get_azkeyvault",
            &["VaultName", "ResourceGroupName", "Location"],
        ),
        (
            "powershell_get_azresourcegroup",
            &["ResourceGroupName", "ProvisioningState", "Location"],
        ),
        (
            "powershell_get_azstorageaccount",
            &["StorageAccountName", "ResourceGroupName", "Kind"],
        ),
        (
            "powershell_get_azsubscription",
            &["SubscriptionId", "TenantId"],
        ),
        (
            "powershell_get_azvm",
            &["ResourceGroupName", "ProvisioningState"],
        ),
        ("powershell_get_chunkdata", &["ChunkId", "DataLength"]),
        ("powershell_get_chunkstore", &["StoreType", "Volume"]),
        (
            "powershell_get_cimclass",
            &["CimClassName", "CimSystemProperties"],
        ),
        ("powershell_get_cimsession", &["ComputerName", "InstanceId"]),
        (
            "powershell_get_container",
            &["Name", "Status", "ContainerName"],
        ),
        ("powershell_get_credential", &["UserName", "Password"]),
        ("powershell_get_disk", &["DiskNumber", "HealthStatus"]),
        ("powershell_get_eventlog", &["Index", "EntryType", "Source"]),
        (
            "powershell_get_history",
            &["CommandLine", "ExecutionStatus"],
        ),
        ("powershell_get_hotfix", &["HotFixID", "InstalledOn"]),
        (
            "powershell_get_installedmodule",
            &["Repository", "InstalledDate"],
        ),
        (
            "powershell_get_localgroup",
            &["PrincipalSource", "SID", "Name"],
        ),
        (
            "powershell_get_localuser",
            &["PrincipalSource", "Enabled", "Name"],
        ),
        (
            "powershell_get_netadapter",
            &["MacAddress", "LinkSpeed", "InterfaceDescription"],
        ),
        (
            "powershell_get_netfirewallrule",
            &["Direction", "Action", "DisplayName", "Enabled"],
        ),
        (
            "powershell_get_netipaddress",
            &["IPAddress", "InterfaceAlias"],
        ),
        ("powershell_get_netroute", &["DestinationPrefix", "NextHop"]),
        (
            "powershell_get_nettcpconnection",
            &["LocalAddress", "RemoteAddress"],
        ),
        (
            "powershell_get_partition",
            &["PartitionNumber", "DriveLetter", "Size"],
        ),
        ("powershell_get_psprovider", &["Drives", "Capabilities"]),
        (
            "powershell_get_pssession",
            &["ComputerName", "ConfigurationName", "State"],
        ),
        ("powershell_get_pssnapin", &["PSVersion", "AssemblyName"]),
        ("powershell_get_scheduledtask", &["TaskName", "TaskPath"]),
        ("powershell_get_service", &["Status", "DisplayName", "Name"]),
        (
            "powershell_get_smbsession",
            &["ClientComputerName", "NumOpens"],
        ),
        ("powershell_get_smbshare", &["ShareState", "ScopeName"]),
        ("powershell_get_vm", &["VMName", "VMId", "CPUUsage"]),
        ("powershell_get_volume", &["DriveLetter", "FileSystemLabel"]),
        ("powershell_get_winevent", &["ProviderName", "TimeCreated"]),
        ("powershell_get_winevent_filter", &[]),
        ("profiles", &["profileIdentifier", "attribute"]),
        ("ps_aux_mac", &["USER", "%CPU", "COMMAND"]),
        ("ps_ef_mac", &["UID", "PPID", "CMD"]),
        ("psloglist", &[]),
        ("pushd_cmd", &[]),
        ("python", &[]),
        ("rg_files", &["/", "."]),
        ("rm", &[]),
        ("rsyslog", &["rsyslogd", "syslog", ":"]),
        ("rsyslog_cli", &["rsyslog", ":"]),
        ("rsyslogd", &["rsyslogd", ":"]),
        ("ruby", &[]),
        ("sc_query", &["SERVICE_NAME", "STATE"]),
        ("scriptrunner", &[]),
        ("setfile", &[]),
        ("sha256sum", &[]),
        ("shutdown_cmd", &[]),
        ("shutdown_win", &[]),
        ("snap", &["Name", "Version"]),
        ("softwareupdate_history", &["Date", "Version"]),
        ("spctl_list_label", &["label", "["]),
        ("ss_a", &["Netid", "Recv-Q"]),
        ("ss_l", &["Netid", "Recv-Q"]),
        ("ss_n", &["Netid", "Recv-Q"]),
        ("ss_p", &["Netid", "Process"]),
        ("ss_t", &["Netid", "Recv-Q"]),
        ("ss_u", &["Netid", "Recv-Q"]),
        ("ssh", &[]),
        ("stat_cmd", &[]),
        ("stat_macos", &[]),
        ("strace_summary", &["% time", "------ ----"]),
        ("strings", &[]),
        ("swapon_show", &["NAME", "PRIO"]),
        ("sysctl", &[":", "="]),
        ("sysctl_short", &[":"]),
        (
            "sysinternals_autorunsc_a",
            &["CLSID", "Publisher", "ImagePath"],
        ),
        (
            "sysinternals_autorunsc_m",
            &["CLSID", "Publisher", "ImagePath"],
        ),
        ("sysinternals_psloglist_after", &[]),
        ("sysinternals_psloglist_filter", &[]),
        ("syslog", &[":"]),
        ("syslog_ng", &["syslog-ng", ":"]),
        ("syslog_show", &[":"]),
        ("syslogd", &["syslogd", ":"]),
        ("syslogd_mac", &["syslogd", ":"]),
        ("system_profiler_short", &[":"]),
        ("systemctl_get_default", &["target", "UNIT"]),
        ("systemctl_list_jobs", &["JOB", "TYPE"]),
        ("systemctl_list_machines", &["MACHINE", "CLASS"]),
        ("systemctl_list_sockets", &["LISTEN", "UNIT"]),
        ("systemctl_list_timers", &["NEXT", "LAST"]),
        ("systemctl_list_unit_files", &["UNIT FILE", "STATE"]),
        ("systemctl_list_units", &["UNIT", "LOAD", "ACTIVE", "SUB"]),
        (
            "systemctl_mask",
            &["masked", "Created symlink", "masked", "/dev/null", "UNIT"],
        ),
        ("systemctl_reset_failed", &[]),
        ("systemctl_set_default", &["target", "UNIT"]),
        (
            "systemctl_unmask",
            &["unmasked", "Removed", "unmasked", "UNIT"],
        ),
        ("systemd_analyze_security", &["UNIT", "EXPOSURE"]),
        ("systemd_cgtop", &["PATH", "TASKS", "CPU"]),
        ("systeminfo", &["Host Name", "OS Name"]),
        ("tar_win", &[]),
        ("tasklist_m", &["Image Name", "Modules"]),
        ("tasklist_svc", &["Image Name", "Services"]),
        ("tasklist_v", &["Image Name", "Status", "PID"]),
        ("tcpdump", &[]),
        (
            "terraform_workspace_delete",
            &["Name", "Deleted", "workspace"],
        ),
        ("terraform_workspace_new", &["Name", "Created", "workspace"]),
        ("terraform_workspace_select", &["Name", "workspace"]),
        ("timeout_cmd", &[]),
        ("top_mac", &["PID", "COMMAND"]),
        ("top_macos_l1", &["PID", "COMMAND"]),
        ("traceroute", &[":", "."]),
        ("traceroute_macos", &[":", "."]),
        ("traceroute_progress", &["traceroute to ", "1  "]),
        ("type_cmd", &[]),
        ("type_win", &[]),
        ("uname", &[]),
        ("uname_macos", &[]),
        ("uname_short", &[]),
        ("uptime_cmd", &[]),
        ("uptime_macos", &[]),
        ("usermod", &[]),
        ("usermod_cmd", &[]),
        ("varnishlog", &[]),
        ("vmstat", &["procs -----", "swpd   free", "r  b   swpd"]),
        ("wall_macos", &[]),
        ("whereis", &[]),
        ("whereis_macos", &[]),
        ("which_macos", &[]),
        ("whoami_macos", &[]),
        ("whoami_win", &[", "]),
        ("whois", &[]),
        ("whois_short", &[]),
        ("winget_list", &["Name", "Id", "Version"]),
        ("winget_search", &["Name", "Id", "Source"]),
        ("winget_upgrade", &["Available", "Name", "Id"]),
        ("wmic", &[]),
        ("write_msg", &[]),
        ("xz", &[]),
        ("youtube_dl", &["[download]", "[youtube]"]),
        ("zeroconf", &["=", ":"]),
        ("zeroconf_cli", &["=", ":"]),
        ("zfs_list", &["NAME", "AVAIL"]),
        ("zip", &[":", "."]),
        ("zpool_iostat", &["capacity", "operations"]),
        ("zpool_list", &["NAME", "HEALTH"]),
    ];

    fn battery(spec: &Spec) -> Vec<String> {
        let needles: Vec<&str> = match spec.detect {
            Detect::ContainsAny(xs) => xs.to_vec(),
            // `All` needs EVERY conjunct present, so seed the header with all.
            Detect::All(ds) => ds
                .iter()
                .filter_map(|d| match d {
                    Detect::ContainsAny(xs) => xs.first().copied(),
                    _ => None,
                })
                .collect(),
            _ => vec!["NAME"],
        };
        let header = format!("{} COL2 COL3", needles.join(" "));
        let table: String = (0..spec.head + 7)
            .map(|i| format!("row{i} val{i} more{i}\n"))
            .collect();
        let prose: String = (0..spec.head + 7)
            .map(|i| format!("a sentence of plain output number {i}\n"))
            .collect();
        let mut out = vec![
            format!("{header}\n{table}"),
            format!("{header}\n{prose}"),
            format!("{header}\nrow0 a b\n"),
            "unrelated output\nwith two lines\n".to_string(),
            String::new(),
            "   \n".to_string(),
        ];
        // ONE input per needle, carrying only THAT needle.
        //
        // Without these the shared header held every needle at once, so a row
        // whose gate was NARROWER or WIDER than its stub still fired on the
        // same inputs and parity passed. MUTATION-TESTED: narrowing
        // `argocd_app_history` from `ContainsAny(&["ID", "DATE"])` to
        // `&["ID"]` was invisible to the old battery and fails this one.
        // That mattered: this harness is the whole evidence for deleting the
        // stub modules it compares against.
        // The rows must REPEAT: a shape with no head/tail fallback returns
        // `None` on a table it declines to fold, and `None == None` hides the
        // very gate difference these inputs exist to expose. MEASURED: with a
        // table of distinct rows the `argocd_app_history` mutation still passed.
        let dup: String = (0..spec.head + 7).map(|_| "same val same\n").collect();
        let frozen = STUB_NEEDLES
            .iter()
            .find(|(k, _)| *k == spec.key)
            .map(|(_, n)| *n)
            .unwrap_or(&[]);
        for n in frozen {
            out.push(format!("{n} COL2 COL3\n{table}"));
            out.push(format!("{n} COL2 COL3\n{dup}"));
        }
        // Shapes the needle-derived inputs above cannot produce. Every input
        // there is a header plus uniform rows, so a row and a module that
        // agree on tables but part ways on key:value records, on a dashed
        // rule, or on a commit-shaped blob looked identical. MEASURED: this
        // block is the whole reason `git_show` and `less` appear.
        out.extend(generic_shapes());
        out.extend(realistic_sample(spec.key));
        out
    }

    /// Output a command really emits, for the rows whose module gates on
    /// content the synthetic inputs above cannot imitate.
    ///
    /// The generic shapes are headers and uniform rows; a module that only
    /// fires on octal modes or on `insmod`-style records declines all of them
    /// and returns `None`, so its row returning `Some` never showed up as a
    /// divergence. MEASURED: the `chmod` row folded 140 lines to 67 while
    /// `linux::chmod` folds them to 13, and every synthetic input agreed.
    /// Add a sample here when a row covers a command with a content gate.
    fn realistic_sample(key: &str) -> Vec<String> {
        let n = 140;
        let body: String = match key {
            "chmod" => (0..n)
                .map(|i| {
                    format!("mode of '/srv/d{}/f{i}.txt' changed from 0644 (rw-r--r--) to 0755 (rwxr-xr-x)\n", i % 11)
                })
                .collect(),
            "lsmod" => (0..n)
                .map(|i| format!("mod_{i} {} 0\n", 1000 + i))
                .collect(),
            "dmesg" => (0..n)
                .map(|i| format!("[ {i}.000000] usb {i}-1: new high-speed USB device\n"))
                .collect(),
            _ => return Vec::new(),
        };
        vec![body]
    }

    /// Content shapes independent of any row's gate.
    fn generic_shapes() -> Vec<String> {
        let kv: String = (0..120)
            .map(|i| format!("key{i}: value {i}\nname: thing {i}\n\n"))
            .collect();
        let flat = (0..200)
            .map(|i| format!("plain row {i} of output"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut dashed = String::from("KEY        VALUE\n---------  ---------\n");
        for i in 0..90 {
            dashed.push_str(&format!("net.k{i}    {i}\n"));
        }
        let assign = (0..300)
            .map(|i| format!("net.ipv4.conf.k{i} = {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let matches = (0..40)
            .map(|i| format!("/path/to/file{i}.rs:{i}:  some matched text here"))
            .collect::<Vec<_>>()
            .join("\n");
        let commit = format!(
            "commit {}\nAuthor: A <a@b.c>\n\ndiff --git a/x b/x\n{}",
            "a".repeat(40),
            (0..150)
                .map(|i| format!("+added line {i}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        // Lines a `Strip` row drops and a head/tail module keeps.
        let noisy = (0..80)
            .map(|i| format!("(see the transcript file for row {i})"))
            .collect::<Vec<_>>()
            .join("\n");
        vec![
            kv,
            flat,
            dashed,
            assign,
            matches,
            commit,
            noisy,
            "single line".into(),
        ]
    }

    #[test]
    fn fold_spec_parity_with_stubs() {
        let live: std::collections::HashSet<&str> =
            crate::cmd::AUTO_DISPATCH_KEYS.iter().copied().collect();
        let mut compared = 0usize;
        let mut diverging: Vec<&str> = Vec::new();
        let mut unroutable: Vec<&str> = Vec::new();
        for spec in FOLD_SPECS {
            // A row keyed on a suffixed filename stem (`arp_win`, `top_mac`)
            // is keyed on something no caller ever passes: `dispatch_auto`
            // strips the suffix, so the program name is `arp`. Such a row is
            // unreachable from either route, and there is no module to compare
            // it against — count it rather than silently skipping it.
            if !live.contains(spec.key) {
                unroutable.push(spec.key);
                continue;
            }
            compared += 1;
            for input in battery(spec) {
                if apply(spec, &input) != crate::cmd::dispatch_auto(spec.key, &input) {
                    diverging.push(spec.key);
                    break;
                }
            }
        }
        assert_eq!(
            compared + unroutable.len(),
            FOLD_SPECS.len(),
            "every row is either compared or counted unroutable"
        );
        // Pinned, not tolerated: each of these is a row no program name can
        // reach, because its key is a filename stem (`arp_win`, `uname_short`)
        // whose command dispatches under the stripped name. They cost binary
        // size and read as coverage. The number is frozen so adding another
        // one is a decision and not an accident.
        //
        // It was 57. The other three were `chmod`, `crmod` and `dsmod`, and
        // they were not stems at all: `build.rs` skipped directory modules
        // with `path.ends_with("mod.rs")`, which is also true of `chmod.rs`.
        // Every row left here now really is a stem.
        assert_eq!(
            unroutable.len(),
            54,
            "rows keyed on a name no caller passes: {unroutable:?}"
        );
        assert_eq!(
            diverging,
            Vec::<&str>::new(),
            "these rows do not reproduce the module `dispatch_auto` routes to, \
             so they shadow it with different output"
        );
    }
}
