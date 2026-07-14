using System;
using System.Collections.Generic;

namespace s3b
{
    abstract public class Job
    {
        protected List<Job> jobs = new List<Job>();

        protected string name = string.Empty;

        virtual public bool run(Model args)
        {
            bool retcode = exec(args);

            return retcode;
        }

        virtual public bool run(Model args, ProcExec.OutputCallback stdout, ProcExec.OutputCallback stderr)
        {
            bool retcode = exec(args, stdout, stderr);

            return retcode;
        }


        protected bool exec(Model args)
        {
            bool retcode = false;
            Config config = Config.getConfig();

            if (!isJobEnabled()) return true;

            Logger.info(name + ": in progress" );

            retcode = (exec(name + ".command", name + ".args") == 0) ? true : false;


            if (retcode)
            {
                Logger.info(name + ": complete");
            }
            else
            {
                Logger.error(name + ": error");
            }

            return retcode;
        }

        protected bool exec(Model args, ProcExec.OutputCallback stdout, ProcExec.OutputCallback stderr)
        {
            bool retcode = false;
            Config config = Config.getConfig();

            if (!isJobEnabled()) return true;

            Logger.info(name + ": in progress");

            retcode = (exec(name + ".command", name + ".args", stdout, stderr) == 0) ? true : false;


            if (retcode)
            {
                Logger.info(name + ": complete");
            }
            else
            {
                Logger.error(name + ": error");
            }

            return retcode;
        }

        protected int exec(string configCmdName, string configArgName)
        { 
            int result = exec(configCmdName, configArgName, null, null);

            return result;
        }

        protected int exec(string configCmdName, string configArgName, ProcExec.OutputCallback stdout, ProcExec.OutputCallback stderr)
        {
            Config config = Config.getConfig();

            string cmd = config.getString(configCmdName);
            string args = config.getString(configArgName);
            ProcExec pe = new ProcExec(cmd, args);
            int result = pe.run(Config.getConfig(), stdout, stderr );

            
            return result;
        }

        public bool isJobEnabled()
        {
            bool result = false;

            int enabled = Config.getConfig().getInt(name + ".enabled");

            if (enabled == 1) result = true;

            return result;
        }

    }
}
