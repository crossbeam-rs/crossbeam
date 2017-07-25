use registry;

#[derive(Debug)]
pub struct Scope {
    _private: (),
}

pub fn pin<F, R>(f: F) -> R
where
    F: FnOnce(&Scope) -> R,
{
    registry::with_current(|participant| {
        // FIXME(stjepang): Pin and unpin this participant.

        let pin = &Scope { _private: () };
        f(pin)
    })
}

pub unsafe fn unprotected<F, R>(f: F) -> R
where
    F: FnOnce(&Scope) -> R,
{
    let pin = &Scope { _private: () };
    f(pin)
}
