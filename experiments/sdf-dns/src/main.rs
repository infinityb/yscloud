use trust_dns_client::rr::LowerName;
use trust_dns_proto::rr::dnssec::SupportedAlgorithms;
use trust_dns_proto::rr::record_type::RecordType;
use trust_dns_server::authority::Authority;
use trust_dns_server::authority::AuthLookup;

fn main() {
    println!("Hello, world!");
}

struct PsqlAuthority {
    //
}

impl Authority for PsqlAuthority {
    type Lookup = AuthLookup;

    type LookupFuture = Pin<Box<dyn Future<Output = Result<Self::Lookup, LookupError>> + Send>>;

    fn lookup(
        &self,
        name: &LowerName,
        rtype: RecordType,
        is_secure: bool,
        supported_algorithms: SupportedAlgorithms,
    ) -> Self::LookupFuture {
        //
    }
}

