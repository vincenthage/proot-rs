use crate::kernel::syscall;
use crate::kernel::{enter, exit};
use crate::process::proot::InfoBag;
use crate::process::tracee::{Tracee, TraceeRestartMethod, TraceeStatus};
use crate::register::{Current, Modified, Original, StackPointer, SysResult, Word};

pub trait SyscallTranslator {
    fn translate_syscall(
        &mut self,
        info_bag: &InfoBag,
        #[cfg(test)] func_syscall_hook: &Option<Box<dyn Fn(&Tracee, bool, bool)>>,
    );
    fn translate_syscall_enter(&mut self, info_bag: &InfoBag);
    fn translate_syscall_exit(&mut self);
}

impl SyscallTranslator for Tracee {
    /// Retrieves the registers,
    /// handles either the enter or exit stage of the system call,
    /// and pushes the registers.
    fn translate_syscall(
        &mut self,
        info_bag: &InfoBag,
        #[cfg(test)] func_syscall_hook: &Option<Box<dyn Fn(&Tracee, bool, bool)>>,
    ) {
        if let Err(error) = self.regs.fetch_regs() {
            error!("proot error: Error while fetching regs: {}", error);
            return;
        }

        let is_sysenter = match self.status {
            TraceeStatus::SysEnter => {
                #[cfg(test)]
                func_syscall_hook
                    .as_ref()
                    .map(|func| func(self, true, true));
                self.translate_syscall_enter(info_bag);
                true
            }
            TraceeStatus::SysExit | TraceeStatus::Error(_) => {
                #[cfg(test)]
                func_syscall_hook
                    .as_ref()
                    .map(|func| func(self, false, true));
                self.translate_syscall_exit();
                false
            }
        };

        if let Err(error) = self.regs.push_regs() {
            error!("proot error: Error while pushing regs: {}", error);
        }

        #[cfg(test)]
        func_syscall_hook
            .as_ref()
            .map(|func| func(self, is_sysenter, false));

        if is_sysenter {
            syscall::print_syscall(self, Current, "sysenter end");
        } else {
            syscall::print_syscall(self, Current, "sysexit end");
        }
    }

    fn translate_syscall_enter(&mut self, info_bag: &InfoBag) {
        // Never restore original register values at the end of this stage.
        self.regs.set_restore_original_regs(false);

        // Saving the original registers here.
        // It is paramount in order to restore the regs after the exit stage,
        // and also as memory in order to remember the original values (like
        // the syscall number, in case this one is changed during the enter stage).
        self.regs.save_current_regs(Original);

        syscall::print_syscall(self, Current, "sysenter start");

        //TODO: notify extensions for SYSCALL_ENTER_START
        // status = notify_extensions(tracee, SYSCALL_ENTER_START, 0, 0);
        // if (status < 0)
        //     goto end;
        // if (status > 0)
        //     return 0;

        let status = enter::translate(info_bag, self);

        //TODO: notify extensions for SYSCALL_ENTER_END event
        // status2 = notify_extensions(tracee, SYSCALL_ENTER_END, status, 0);
        // if (status2 < 0)
        //     status = status2;

        // Saving the registers potentially modified by the translation.
        // It's useful in order to know what the translation did to the registers.
        self.regs.save_current_regs(Modified);

        // In case of error reported by the translation/extension,
        // remember the tracee status for the "exit" stage and avoid
        // the actual syscall.
        if let Err(error) = status {
            debug!("translate_syscall_enter: {}", error);
            self.regs
                .cancel_syscall("Error in enter stage, avoid syscall");
            self.regs.set(
                SysResult,
                (-(error.get_errno() as i32)) as Word,
                "Error in enter stage, record errno for exit stage",
            );
            self.status = TraceeStatus::Error(error);
        } else {
            self.status = TraceeStatus::SysExit;
        }

        // Restore tracee's stack pointer now if it won't hit
        // the sysexit stage (i.e. when seccomp is enabled and
        // there's nothing else to do).
        if self.restart_how == TraceeRestartMethod::WithoutExitStage {
            self.status = TraceeStatus::SysEnter;
            self.regs.restore_original(
                StackPointer,
                "following enter stage, restoring stack pointer early because no exit stage",
            );
        }
    }

    fn translate_syscall_exit(&mut self) {
        // By default, restore original register values at the end of this stage.
        self.regs.set_restore_original_regs(true);

        syscall::print_syscall(self, Current, "sysexit start");

        //TODO: notify extensions for SYSCALL_EXIT_START event
        // status = notify_extensions(tracee, SYSCALL_EXIT_START, 0, 0);
        // if (status < 0) {
        //     poke_reg(tracee, SYSARG_RESULT, (word_t) status);
        //     goto end;
        // }
        // if (status > 0)
        //     return;

        if self.status.is_ok() {
            exit::translate(self);
        } else {
            self.regs.set(
                SysResult,
                (-(self.status.get_errno() as i32)) as Word,
                "Following previous error in enter stage, setting errno",
            );
        }

        //TODO: notify extensions for SYSCALL_EXIT_END event
        // status = notify_extensions(tracee, SYSCALL_EXIT_END, 0, 0);
        // if (status < 0)
        //     poke_reg(tracee, SYSARG_RESULT, (word_t) status);

        // reset the tracee's status
        self.status = TraceeStatus::SysEnter;
    }
}
