//! The Task Scheduler definition — Windows' supervisor for the proxy.

use super::*;

pub(super) fn windows_task_xml(svc: &Service) -> String {
    let user_id = windows_principal_user();
    let argv = svc.polling_argv();
    let (exe, args) = argv.split_first().expect("argv is never empty");
    // Quote the arguments the way Windows parses them, and quote the command
    // separately: `Command` and `Arguments` are distinct fields, so an exe path
    // with a space does not need quoting here but an argument containing one
    // does.
    let args = args
        .iter()
        .map(|a| {
            if a.contains(' ') {
                format!("\"{a}\"")
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-16\"?>\n\
         <Task version=\"1.2\" xmlns=\"http://schemas.microsoft.com/windows/2004/02/mit/task\">\n\
         \x20 <RegistrationInfo>\n\
         \x20   <Description>tokenade LLM proxy (ladder rank 4)</Description>\n\
         \x20 </RegistrationInfo>\n\
         \x20 <Triggers>\n\
         \x20   <LogonTrigger><Enabled>true</Enabled></LogonTrigger>\n\
         \x20   <TimeTrigger>\n\
         \x20     <StartBoundary>2000-01-01T00:00:00</StartBoundary>\n\
         \x20     <Enabled>true</Enabled>\n\
         \x20     <Repetition>\n\
         \x20       <Interval>PT1M</Interval>\n\
         \x20       <StopAtDurationEnd>false</StopAtDurationEnd>\n\
         \x20     </Repetition>\n\
         \x20   </TimeTrigger>\n\
         \x20 </Triggers>\n\
         \x20 <!-- `S4U` and not `InteractiveToken`, and that is the difference\n\
         \x20      between a supervisor and a decoration. Measured 2026-08-21 on a\n\
         \x20      Windows VM: with `InteractiveToken`, `schtasks /query /v` answers\n\
         \x20      \"Logon Mode: Interactive only\" and \"Last Result: 267011\"\n\
         \x20      (never run) on a machine where nobody is at the console — the\n\
         \x20      task is registered, enabled, and starts nothing. The \"proxy\n\
         \x20      survives a kill\" column read 24/24 `Ko proxy` for that reason\n\
         \x20      alone. The SAME task, with only this field changed, answers\n\
         \x20      \"Interactive/Background\", \"Last Result: 0\", and the proxy\n\
         \x20      runs in the `Services` session. `S4U` needs no stored password:\n\
         \x20      the scheduler service opens the session on behalf of the\n\
         \x20      user. -->\n\
         \x20 <Principals>\n\
         \x20   <Principal id=\"Author\">\n\
         {user_id}\
         \x20     <LogonType>S4U</LogonType>\n\
         \x20     <RunLevel>LeastPrivilege</RunLevel>\n\
         \x20   </Principal>\n\
         \x20 </Principals>\n\
         \x20 <Settings>\n\
         \x20   <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>\n\
         \x20   <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>\n\
         \x20   <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>\n\
         \x20   <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>\n\
         \x20 </Settings>\n\
         \x20 <Actions Context=\"Author\">\n\
         \x20   <Exec>\n\
         \x20     <Command>{}</Command>\n\
         \x20     <Arguments>{}</Arguments>\n\
         \x20   </Exec>\n\
         \x20 </Actions>\n\
         </Task>\n",
        xml_escape(exe),
        xml_escape(&args)
    )
}
