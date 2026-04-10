use crate::cli::RtcCommand;
use ec_test_lib::RtcSource;

pub fn run<S: RtcSource>(source: S, cmd: RtcCommand) -> Result<(), S::Error> {
    match cmd {
        RtcCommand::GetCapabilities => println!("{:?}", source.get_capabilities()?),
        RtcCommand::GetRealTime => println!("{:?}", source.get_real_time()?),
        RtcCommand::GetWakeStatus { timer } => println!("{:?}", source.get_wake_status(timer.into())?),
        RtcCommand::GetExpiredTimerWakePolicy { timer } => {
            println!("{:?}", source.get_expired_timer_wake_policy(timer.into())?)
        }
        RtcCommand::GetTimerValue { timer } => println!("{:?}", source.get_timer_value(timer.into())?),
    }
    Ok(())
}
