use crate::msg::MsgData;
use crate::sys::cq;

pub(crate) fn next(state: &mut cq::OperationState) -> Option<MsgData> {
    let result = match state {
        cq::OperationState::Single { result } => *result,
        cq::OperationState::Multishot { results } if results.is_empty() => return None,
        cq::OperationState::Multishot { results } => results.remove(0),
    };
    Some(result.as_msg())
}
