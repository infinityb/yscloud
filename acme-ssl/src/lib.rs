use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Instant;

use ::log::{debug, log, warn};
use openssl::error::ErrorStack;
use openssl::pkey::{PKey, Private};
use openssl::ssl::{NameType, SniError, SslAcceptor, SslContext, SslMethod, SslRef};
use openssl::x509::X509;

pub struct SslContextAcquired {
    // based on what our keying agent says - supposed be before actual expiration
    // but liable to be much sooner.
    //
    // in reality, it's hard to guarantee the above, since the system time
    // can change drastically.
    expiration: Instant,
    context: SslContext,
}

impl SslContextAcquired {
    fn is_valid(&self) -> bool {
        self.expiration < Instant::now()
    }
}

pub struct SslMultiContext {
    contexts: Arc<RwLock<HashMap<String, SslContextAcquired>>>,
}

fn set_context_by_hostname(
    contexts: &HashMap<String, SslContextAcquired>,
    ssl: &mut SslRef,
    servername: &str,
) -> Result<(), SniError> {
    debug!("servername = {:?}", servername);
    if let Some(w) = contexts.get(servername) {
        if w.is_valid() {
            debug!("found valid SSL context for {:?}", servername);
            ssl.set_ssl_context(&w.context).map_err(|stack| {
                warn!("failed to set context: {}", stack);
                SniError::ALERT_FATAL
            })?;
            return Ok(());
        } else {
            warn!(
                "found invalid SSL context for {:?} - refresher not working?",
                servername
            );
        }
    }

    warn!("invalid hostname {:?}", servername);
    Err(SniError::ALERT_FATAL)
}

pub struct AddCertRequest<'a> {
    name: String,
    stack: &'a [X509],
    pkey: &'a PKey<Private>,
    expiration: Instant,
}

impl SslMultiContext {
    pub fn add_certificate(&self, req: AddCertRequest) -> Result<(), ErrorStack> {
        let mut builder = SslAcceptor::mozilla_modern(SslMethod::tls()).unwrap();

        let mut stack_iter = req.stack.iter();
        builder.set_private_key(req.pkey)?;
        builder.set_certificate(stack_iter.next().unwrap())?;
        for cert in stack_iter {
            builder.add_extra_chain_cert(cert.clone())?;
        }

        pub struct SslAcceptorFake(SslContext);

        let context: SslAcceptorFake = unsafe { ::std::mem::transmute(builder.build()) };

        let mut contexts = self.contexts.write().unwrap();
        contexts.insert(
            req.name,
            SslContextAcquired {
                expiration: req.expiration,
                context: context.0,
            },
        );

        Ok(())
    }

    pub fn build(&self) -> Result<SslAcceptor, ErrorStack> {
        let mut builder = SslAcceptor::mozilla_modern(SslMethod::tls()).unwrap();

        let ctxs = Arc::clone(&self.contexts);
        builder.set_servername_callback(move |ssl, _alert| {
            let servername: String = ssl
                .servername(NameType::HOST_NAME)
                .map(|x| x.to_string())
                .ok_or_else(|| SniError::ALERT_FATAL)?;

            let ctxs = ctxs.read().unwrap();
            set_context_by_hostname(&ctxs, ssl, &servername)?;
            Ok(())
        });

        Ok(builder.build())
    }
}
