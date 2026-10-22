//! A GOG launch: the installed build's primary play task, natively or in its
//! Wine prefix. GOG's builds need no account session to start.
use super::*;
use crate::supervisor::{
    Target,
    architecture::Architecture,
    session::{self as lifecycle, LaunchCloud, Opening, Prepared, Session},
    storage,
};
use std::{
    io::{Read, Write},
    sync::Arc,
};

pub(crate) struct Launch {
    request: Request,
}

impl lifecycle::Launch for Launch {
    type P = Gog;
    type Host = ();
    type Cloud = Box<dyn LaunchCloud<Error>>;

    fn wine(&self) -> Option<crate::wine::Config> {
        self.request.wine.clone()
    }

    fn open(input: &mut dyn Read) -> Result<(Opening<Gog>, Self)> {
        let request: Request = storage::packet(input)?;
        request.config.validate(&request.account_id)?;
        if uuid::Uuid::parse_str(&request.id).is_err()
            || !request.config.runs_in(request.wine.as_ref())
        {
            return Err(Error::Session);
        }
        let opening = Opening {
            root: request.config.session_directory.clone(),
            id: request.id.clone(),
            account_id: request.account_id.clone(),
            store: Store {
                product_id: request.config.product_id.clone(),
            },
            helpers: crate::supervisor::rosetta::helpers_of_host(&request.config.helper)
                .ok_or(Error::Session)?
                .into(),
        };
        Ok((opening, Self { request }))
    }

    async fn cloud(&mut self, _: &Arc<Session<Gog>>) -> Result<Option<Self::Cloud>> {
        Ok(None)
    }

    async fn target(&mut self, _: &Arc<Session<Gog>>) -> Result<(Target, Option<Architecture>)> {
        let request = &self.request;
        let installed = gog_kit::client::installed_launch(
            &request.config.product_id,
            request.config.platform,
            &request.config.install,
        )?;
        let (target, architecture) = match &request.wine {
            Some(wine) => windows_target(
                &installed.target,
                &request.config,
                wine,
                &request.environment,
            )?,
            None => native_target(&installed.target, &request.config, &request.environment)?,
        };
        Ok((target, Some(architecture)))
    }

    async fn prepare_host(
        &mut self,
        _: &Arc<Session<Gog>>,
        target: &Target,
        _: Option<Architecture>,
        _: Option<&Self::Cloud>,
    ) -> Result<Prepared<()>> {
        Ok(Prepared {
            host: (),
            helper: self.request.config.helper.clone(),
            subcommand: "game-exec",
            environment: target.environment.clone(),
        })
    }

    fn running(&self, _: &Arc<()>, _: u32) -> Box<dyn Fn(bool) + Send + Sync> {
        Box::new(|_| {})
    }

    fn stopped(&self, _: &(), _: Option<i32>) {}

    /// No Cloud sync settles a GOG game's saves.
    fn terminal(record: &mut Snapshot) {
        record.cloud.allowed_choices.clear();
        record.cloud.phase = crate::cloud::Phase::Unsupported;
    }
}

/// A Windows build, which Wine starts in its prefix.
fn windows_target(
    launched: &obelisk_core::launch::LaunchTarget,
    config: &Configuration,
    wine: &crate::wine::Config,
    environment: &std::collections::HashMap<String, String>,
) -> Result<(Target, Architecture)> {
    let target = crate::wine::target::<super::windows::Prefix>(
        wine,
        &config.game(),
        launched,
        environment.clone(),
    )?;
    Ok((target, wine.architecture()))
}

fn native_target(
    launched: &obelisk_core::launch::LaunchTarget,
    config: &Configuration,
    environment: &std::collections::HashMap<String, String>,
) -> Result<(Target, Architecture)> {
    Ok(crate::native::target(
        &config.install,
        launched,
        launched.arguments.clone(),
        environment.clone(),
        crate::diagnostics::game_log(
            &config.home,
            config.profile_game().into(),
            obelisk_core::Platform::Mac,
        ),
    )?)
}

/// The `obelisk-steam-host gog-supervise` entry point.
pub fn serve(input: impl Read, output: impl Write, inspect: lifecycle::Inspect) -> Result<()> {
    lifecycle::serve::<Launch>(input, output, inspect)
}

#[cfg(test)]
mod tests;
