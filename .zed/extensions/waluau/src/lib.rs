use zed::settings::LspSettings;
use zed_extension_api::{self as zed, LanguageServerId, Result};

struct WaluauExtension;

impl WaluauExtension {
    fn language_server_binary(
        worktree: &zed::Worktree,
        settings: &LspSettings,
    ) -> Result<zed::Command> {
        let configured = settings.binary.as_ref();
        let command = configured
            .and_then(|binary| binary.path.clone())
            .unwrap_or_else(|| {
                format!(
                    "{}/tools/editors/waluau-lsp",
                    worktree.root_path().trim_end_matches(['/', '\\'])
                )
            });

        if configured.and_then(|binary| binary.path.as_ref()).is_none() {
            worktree
                .read_text_file("tools/editors/waluau-lsp")
                .map_err(|_| {
                    "waluau-lsp launcher not found; open the Waluau repository or configure lsp.waluau-lsp.binary.path"
                        .to_string()
                })?;
        }

        Ok(zed::Command {
            command,
            args: configured
                .and_then(|binary| binary.arguments.clone())
                .unwrap_or_default(),
            env: configured
                .and_then(|binary| binary.env.clone())
                .unwrap_or_default()
                .into_iter()
                .collect(),
        })
    }
}

impl zed::Extension for WaluauExtension {
    fn new() -> Self {
        Self
    }

    fn language_server_command(
        &mut self,
        _language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        let settings = LspSettings::for_worktree("waluau-lsp", worktree)?;
        Self::language_server_binary(worktree, &settings)
    }

    fn language_server_initialization_options(
        &mut self,
        _language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<Option<zed::serde_json::Value>> {
        Ok(LspSettings::for_worktree("waluau-lsp", worktree)?.initialization_options)
    }

    fn language_server_workspace_configuration(
        &mut self,
        _language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<Option<zed::serde_json::Value>> {
        Ok(LspSettings::for_worktree("waluau-lsp", worktree)?.settings)
    }
}

zed::register_extension!(WaluauExtension);
