use super::{common_tail, escape, footer, header, NewError, NewRequest};

pub(super) fn xml(request: &NewRequest, project: &str) -> Result<String, NewError> {
    if !request.sets.is_empty() {
        return Err(NewError::Invalid("directory-service does not accept cells or credentials; set the admin password in Composer".into()));
    }
    let (class, mut tables) = match request.directory_type.as_deref() {
        Some("ldap") => (
            "LDAPDirectoryService",
            include_str!("fixtures/directory-ldap.xml")
                .trim()
                .to_string(),
        ),
        // An import with an empty UserProvisioningExclusionList comes back with Administrator
        // in it (10.2); start with that row so the read-back compares equal.
        Some("active-directory") => {
            let mut tables = include_str!("fixtures/directory-active-directory.xml")
                .trim()
                .to_string();
            let table = tables
                .find("name=\"UserProvisioningExclusionList\"")
                .expect("exclusion list in the fixture");
            let rows = table + tables[table..].find("<Rows/>").expect("its empty rows");
            tables.replace_range(
                rows..rows + "<Rows/>".len(),
                "<Rows><Row><thingworxUserName><![CDATA[Administrator]]></thingworxUserName></Row></Rows>",
            );
            ("ActiveDirectoryDirectoryService", tables)
        }
        _ => {
            return Err(NewError::Invalid(
                "directory-service --type must be ldap or active-directory".into(),
            ))
        }
    };
    let priority = request
        .priority
        .as_deref()
        .unwrap_or("1")
        .parse::<i32>()
        .map_err(|_| NewError::Invalid("--priority must be an INTEGER".into()))?;
    if let Some(port) = &request.port {
        let port = port
            .parse::<u16>()
            .ok()
            .filter(|port| *port > 0)
            .ok_or_else(|| NewError::Invalid("--port must be between 1 and 65535".into()))?;
        let default = if class == "LDAPDirectoryService" {
            "10389.0"
        } else {
            "389"
        };
        let value = if class == "LDAPDirectoryService" {
            format!("{port}.0")
        } else {
            port.to_string()
        };
        let cell = format!("<port>{default}</port>");
        assert!(tables.contains(&cell), "directory fixture default port");
        tables = tables.replacen(&cell, &format!("<port>{value}</port>"), 1);
    }
    if let Some(server) = &request.server {
        let host_char = |c: char| c.is_ascii_alphanumeric() || ".-_:[]".contains(c);
        if server.is_empty() || !server.chars().all(host_char) {
            return Err(NewError::Invalid(
                "--host needs a host name or IP address (letters, digits, . - _ : [ ])".into(),
            ));
        }
        // CDATA like the server's own export, so the read-back compares equal.
        let cell = "<server><![CDATA[localhost]]></server>";
        assert!(tables.contains(cell), "directory fixture default host");
        tables = tables.replacen(cell, &format!("<server><![CDATA[{server}]]></server>"), 1);
    }
    Ok(format!("{}        <DirectoryService className=\"com.thingworx.security.directoryservices.{class}\" description=\"{}\" documentationContent=\"\" enabled=\"false\" homeMashup=\"\" name=\"{}\" priority=\"{priority}\" projectName=\"{}\" tags=\"\">\n{}\n            <ConfigurationTableDefinitions/>\n            {tables}\n        </DirectoryService>\n{}", header("DirectoryServices"), escape(&request.description)?, escape(&request.name)?, escape(project)?, common_tail(&[])?.replace("            <ConfigurationTableDefinitions></ConfigurationTableDefinitions>\n            <ConfigurationTables></ConfigurationTables>", ""), footer("DirectoryServices")))
}
