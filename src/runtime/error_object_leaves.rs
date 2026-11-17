//! Test-only: one sample of every leaf variant of every tagged enum reachable from
//! [`DispatchError`], **compiler-enforced**. Each enum's samples come from a wildcard-free
//! successor `match` (`next_*`): walking from `first` visits arm after arm, so a new variant
//! fails to compile until it gets an arm — and that arm *is* its table entry. An internally
//! tagged newtype over a bare `String` cannot serialize (it used to surface as
//! `kind: "internal"`), which is what `error_object_tests` checks every leaf for.

use crate::http::{
    AuthError, BindError, BodyError, CallError, CredError, GuardError, PaginateError, TemplateError,
};
use crate::project::ProjectionError;
use crate::runtime::dispatch::DispatchError;
use crate::schema::ValidationError;

fn s(v: &str) -> String {
    v.to_owned()
}

fn walk<T>(first: T, next: fn(&T) -> Option<T>) -> Vec<T> {
    let mut out = vec![first];
    while let Some(n) = out.last().and_then(next) {
        out.push(n);
    }
    out
}

fn next_template(e: &TemplateError) -> Option<TemplateError> {
    use TemplateError::*;
    let (t, seg, ph) = (s("t"), s("x"), s("id"));
    match e {
        NotAbsolute { .. } => Some(DoubleSlashStart { template: t }),
        DoubleSlashStart { .. } => Some(ContainsQueryOrFragment { template: t }),
        ContainsQueryOrFragment { .. } => Some(ContainsSchemeSeparator { template: t }),
        ContainsSchemeSeparator { .. } => Some(MixedSegment { segment: seg }),
        MixedSegment { .. } => Some(DotSegment { segment: seg }),
        DotSegment { .. } => Some(EmptySegment),
        EmptySegment => Some(DuplicatePlaceholder { placeholder: ph }),
        DuplicatePlaceholder { .. } => Some(MalformedPercentEncoding { segment: seg }),
        MalformedPercentEncoding { .. } => Some(MissingValue { placeholder: ph }),
        MissingValue { .. } => Some(EmptyValue { placeholder: ph }),
        EmptyValue { .. } => Some(DotValue { placeholder: ph }),
        DotValue { .. } => Some(ControlCharacter { placeholder: ph }),
        ControlCharacter { .. } => None,
    }
}

fn next_bind(e: &BindError) -> Option<BindError> {
    use BindError::*;
    match e {
        Template(_) => Some(HeaderNotAllowed { name: s("h") }),
        HeaderNotAllowed { .. } => Some(HeaderInvalidValue { name: s("h") }),
        HeaderInvalidValue { .. } => Some(UnsupportedValueShape {
            name: s("p"),
            shape: "array",
        }),
        UnsupportedValueShape { .. } => Some(LocalParamNotBindable { name: s("p") }),
        LocalParamNotBindable { .. } => Some(BodyAssign {
            pointer: s("/a"),
            message: s("m"),
        }),
        BodyAssign { .. } => Some(NoHost),
        NoHost => Some(BaseUrlHasQueryOrFragment),
        BaseUrlHasQueryOrFragment => Some(UrlParse { message: s("m") }),
        UrlParse { .. } => Some(ContainmentViolation),
        ContainmentViolation => None,
    }
}

fn next_validation(e: &ValidationError) -> Option<ValidationError> {
    use ValidationError::*;
    match e {
        NotAnObject => Some(UnknownParam { param: s("p") }),
        UnknownParam { .. } => Some(MissingRequired { param: s("p") }),
        MissingRequired { .. } => Some(WrongType {
            param: s("p"),
            expected: "number",
            got: "string",
        }),
        WrongType { .. } => Some(NotInEnum { param: s("p") }),
        NotInEnum { .. } => None,
    }
}

fn next_guard(e: &GuardError) -> Option<GuardError> {
    use GuardError::*;
    match e {
        UnsupportedScheme { .. } => Some(OriginNotAllowed { origin: s("o") }),
        OriginNotAllowed { .. } => Some(NoHost),
        NoHost => Some(DeniedLiteralIp {
            host: s("10.0.0.1"),
            range: "private",
        }),
        DeniedLiteralIp { .. } => None,
    }
}

fn next_cred(e: &CredError) -> Option<CredError> {
    use CredError::*;
    match e {
        MissingEnvVar { .. } => Some(InvalidHeaderValue),
        InvalidHeaderValue => Some(MissingStoredCredential),
        MissingStoredCredential => None,
    }
}

fn next_auth(e: &AuthError) -> Option<AuthError> {
    match e {
        AuthError::Credential(_) => Some(AuthError::InvalidHeaderName { name: s("h") }),
        AuthError::InvalidHeaderName { .. } => None,
    }
}

fn next_body(e: &BodyError) -> Option<BodyError> {
    use BodyError::*;
    match e {
        ContentLengthExceedsCap { .. } => Some(StreamExceedsCap { cap: 1 }),
        StreamExceedsCap { .. } => Some(Transport { message: s("m") }),
        Transport { .. } => None,
    }
}

fn next_call(e: &CallError) -> Option<CallError> {
    use CallError::*;
    match e {
        Guard(_) => Some(Auth(AuthError::InvalidHeaderName { name: s("h") })),
        Auth(_) => Some(Body(BodyError::StreamExceedsCap { cap: 1 })),
        Body(_) => Some(TooManyRedirects { max: 5 }),
        TooManyRedirects { .. } => Some(MissingLocation),
        MissingLocation => Some(InvalidLocation { reason: s("r") }),
        InvalidLocation { .. } => Some(Timeout),
        Timeout => Some(InvalidHeader),
        InvalidHeader => Some(Transport { message: s("m") }),
        Transport { .. } => None,
    }
}

fn next_paginate(e: &PaginateError) -> Option<PaginateError> {
    use PaginateError::*;
    match e {
        Call(_) => Some(PageCapExceeded { max: 3 }),
        PageCapExceeded { .. } => Some(ResponseNotJson { message: s("m") }),
        ResponseNotJson { .. } => Some(CursorNotString {
            pointer: s("/next"),
            actual: "number",
        }),
        CursorNotString { .. } => None,
    }
}

fn next_projection(e: &ProjectionError) -> Option<ProjectionError> {
    use ProjectionError::*;
    let (name, path) = (s("f"), s("$.a"));
    match e {
        InvalidPath { .. } => Some(Missing { name, path }),
        Missing { .. } => Some(AmbiguousMatch {
            name,
            path,
            count: 2,
        }),
        AmbiguousMatch { .. } => Some(CoercionFailed {
            name,
            actual: "string",
            expected: "number",
        }),
        CoercionFailed { .. } => None,
    }
}

fn next_dispatch(e: &DispatchError) -> Option<DispatchError> {
    use DispatchError::*;
    match e {
        NotDeclared { .. } => Some(NotAnApiCall { name: s("a") }),
        NotAnApiCall { .. } => Some(Args(ValidationError::NotAnObject)),
        Args(_) => Some(Bind(BindError::NoHost)),
        Bind(_) => Some(Send(PaginateError::PageCapExceeded { max: 3 })),
        Send(_) => Some(Client { message: s("m") }),
        Client { .. } => Some(HttpStatus {
            status: 404,
            reason: s("Not Found"),
            detail: s(""),
            attempt: None,
        }),
        HttpStatus { .. } => Some(ResponseNotJson {
            message: s("m"),
            attempt: None,
        }),
        ResponseNotJson { .. } => Some(Projection {
            source: ProjectionError::Missing {
                name: s("f"),
                path: s("$"),
            },
            attempt: None,
        }),
        Projection { .. } => None,
    }
}

/// The `kind` each top-level variant must serialize to — also wildcard-free.
pub fn expected_kind(e: &DispatchError) -> &'static str {
    use DispatchError::*;
    match e {
        NotDeclared { .. } => "not_declared",
        NotAnApiCall { .. } => "not_an_api_call",
        Args(_) => "args",
        Bind(_) => "bind",
        Send(_) => "send",
        Client { .. } => "client",
        HttpStatus { .. } => "http_status",
        ResponseNotJson { .. } => "response_not_json",
        Projection { .. } => "projection",
    }
}

/// Every leaf, each nested enum's chain spliced in under every wrapper variant that carries it.
pub fn every_leaf() -> Vec<DispatchError> {
    let template = walk(
        TemplateError::NotAbsolute { template: s("t") },
        next_template,
    );
    let mut bind = walk(BindError::Template(TemplateError::EmptySegment), next_bind);
    bind.extend(template.into_iter().map(BindError::Template));

    let cred = walk(CredError::MissingEnvVar { env_key: s("K") }, next_cred);
    let mut auth = walk(
        AuthError::Credential(CredError::InvalidHeaderValue),
        next_auth,
    );
    auth.extend(cred.into_iter().map(AuthError::Credential));

    let guard = walk(
        GuardError::UnsupportedScheme { scheme: s("ftp") },
        next_guard,
    );
    let body = walk(
        BodyError::ContentLengthExceedsCap { cap: 1, actual: 2 },
        next_body,
    );
    let mut call = walk(CallError::Guard(GuardError::NoHost), next_call);
    call.extend(guard.into_iter().map(CallError::Guard));
    call.extend(auth.into_iter().map(CallError::Auth));
    call.extend(body.into_iter().map(CallError::Body));

    let mut paginate = walk(PaginateError::Call(CallError::Timeout), next_paginate);
    paginate.extend(call.into_iter().map(PaginateError::Call));

    let projection = walk(
        ProjectionError::InvalidPath {
            name: s("f"),
            path: s("$"),
            message: s("m"),
        },
        next_projection,
    );

    let mut all = walk(DispatchError::NotDeclared { name: s("a") }, next_dispatch);
    all.extend(
        walk(ValidationError::NotAnObject, next_validation)
            .into_iter()
            .map(DispatchError::Args),
    );
    all.extend(bind.into_iter().map(DispatchError::Bind));
    all.extend(paginate.into_iter().map(DispatchError::Send));
    all.extend(
        projection
            .into_iter()
            .map(|source| DispatchError::Projection {
                source,
                attempt: None,
            }),
    );
    all
}
