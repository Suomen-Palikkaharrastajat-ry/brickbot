use serenity::all::{CommandInteraction, ComponentInteraction, Context, ModalInteraction};

#[derive(Debug)]
pub enum WorkflowError {
    Unauthorized,
    Expired,
    Validation,
    ExternalUnavailable,
    Internal,
}

impl std::fmt::Display for WorkflowError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for WorkflowError {}

impl WorkflowError {
    #[must_use]
    pub fn localized_message(&self, locale: &str) -> String {
        match self {
            Self::Unauthorized => rust_i18n::t!("errors.unauthorized", locale = locale).to_string(),
            Self::Expired => rust_i18n::t!("errors.expired", locale = locale).to_string(),
            Self::Validation => rust_i18n::t!("errors.validation", locale = locale).to_string(),
            Self::ExternalUnavailable => {
                rust_i18n::t!("errors.external_unavailable", locale = locale).to_string()
            }
            Self::Internal => rust_i18n::t!("errors.internal", locale = locale).to_string(),
        }
    }
}

pub struct InteractionAdapter<'a> {
    pub ctx: &'a Context,
    pub command: Option<&'a CommandInteraction>,
    pub component: Option<&'a ComponentInteraction>,
    pub modal: Option<&'a ModalInteraction>,
    pub deferred: std::sync::atomic::AtomicBool,
}

impl<'a> InteractionAdapter<'a> {
    #[must_use]
    pub const fn from_command(ctx: &'a Context, command: &'a CommandInteraction) -> Self {
        Self {
            ctx,
            command: Some(command),
            component: None,
            modal: None,
            deferred: std::sync::atomic::AtomicBool::new(false),
        }
    }

    #[must_use]
    pub const fn from_component(ctx: &'a Context, component: &'a ComponentInteraction) -> Self {
        Self {
            ctx,
            command: None,
            component: Some(component),
            modal: None,
            deferred: std::sync::atomic::AtomicBool::new(false),
        }
    }

    #[must_use]
    pub const fn from_modal(ctx: &'a Context, modal: &'a ModalInteraction) -> Self {
        Self {
            ctx,
            command: None,
            component: None,
            modal: Some(modal),
            deferred: std::sync::atomic::AtomicBool::new(false),
        }
    }

    pub async fn defer_ephemeral(&self) -> serenity::Result<()> {
        if self
            .deferred
            .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            return Ok(()); // Already deferred
        }
        if let Some(cmd) = self.command {
            cmd.defer_ephemeral(&self.ctx.http).await
        } else if let Some(comp) = self.component {
            comp.defer_ephemeral(&self.ctx.http).await
        } else if let Some(modal) = self.modal {
            modal.defer_ephemeral(&self.ctx.http).await
        } else {
            Ok(())
        }
    }

    pub async fn complete(&self, msg: &str) -> serenity::Result<()> {
        let resp = serenity::builder::EditInteractionResponse::new().content(msg);
        if let Some(cmd) = self.command {
            let _ = cmd.edit_response(&self.ctx.http, resp).await;
        } else if let Some(comp) = self.component {
            let _ = comp.edit_response(&self.ctx.http, resp).await;
        } else if let Some(modal) = self.modal {
            let _ = modal.edit_response(&self.ctx.http, resp).await;
        }
        Ok(())
    }

    pub async fn fail_localized(&self, err: WorkflowError, locale: &str) -> serenity::Result<()> {
        let msg = err.localized_message(locale);
        tracing::error!("Interaction failed: {:?}", err);
        let resp = serenity::builder::EditInteractionResponse::new().content(msg);
        if let Some(cmd) = self.command {
            let _ = cmd.edit_response(&self.ctx.http, resp).await;
        } else if let Some(comp) = self.component {
            let _ = comp.edit_response(&self.ctx.http, resp).await;
        } else if let Some(modal) = self.modal {
            let _ = modal.edit_response(&self.ctx.http, resp).await;
        }
        Ok(())
    }
}
