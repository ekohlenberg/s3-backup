using System;
using System.Collections.Generic;
using System.IO;
using System.Reflection;
namespace s3b
{
    class Program
    {
        
        public class UsageException : Exception
        {
            public UsageException(string message) : base(message + "\ns3b\t-action backup -folder <backup_folder> -bucket <s3_bucket>\n\t-action restore -bucket <s3_bucket> [-object <object>]\n")
            {
            }
        }

        static int Main(string[] args)
        {
            int retcode = 1;
            PersistBase.Persistence = new SqlitePersist(new s3bSqliteTemplate());
            Logger.Persist = PersistBase.Persistence;

            try
            {
                Job job = parse(args);

                if (job != null)
                {
                    retcode = (job.run(Config.getConfig()))? 0 : 1;
                }

            }
            catch (UsageException u)
            {
                Logger.info(u.Message);
                retcode = 1;
            }
            catch (Exception e)
            {
                Logger.error(e);
                retcode = 1;
            }

            return retcode;
        }

        public class ReqdParam
        {
            public string param;
            public string message;

            public ReqdParam(string p, string m)
            {
                param = p;
                message = m;
            }

        }

        static Job parse(string[] args)
        {
            Job job = null;

            
            Dictionary<string, Job> jobs = addActions();

            Dictionary<string, List<ReqdParam>> validationRules = addValidations();
            parseArgs(args);

            if (!Config.getConfig().ContainsKey("action")) throw new UsageException("-action not defined");

            string action;

            getAction(out job, jobs, out action);

            validateAction(validationRules, action);

            Config.getConfig().setValue("temp", Config.getConfig().getString("s3b.temp"));


            return job;
        }

        private static void parseArgs(string[] args)
        {
            string currentKey = string.Empty;
            string currentValue = string.Empty;

            if (args.Length == 0) throw new UsageException("");
            foreach (string arg in args)
            {
                if (arg.StartsWith("-"))
                {
                    currentKey = arg.Substring(1);
                }
                else
                {
                    if (currentKey.Length == 0) throw new UsageException("");
                    currentValue = arg;
                    Config.getConfig().Add(currentKey, currentValue);
                }
            }
        }

        private static void getAction(out Job job, Dictionary<string, Job> jobs, out string action)
        {
            action = Config.getConfig().getString("action");
            if ((action != "backup") && (action != "restore")) throw new UsageException("Action " + action + " not supported. Only actions supported are backup and restore. ");

            job = jobs[action];
        }

        private static void validateAction(Dictionary<string, List<ReqdParam>> validationRules, string action)
        {
            List<ReqdParam> rules = validationRules[action];

            foreach (ReqdParam r in rules)
            {
                if (!Config.getConfig().ContainsKey(r.param)) throw new UsageException(r.message);
            }
        }

        private static Dictionary<string, List<ReqdParam>> addValidations()
        {
            // backup required parameters
            List<ReqdParam> backupParams = new List<ReqdParam>();
            backupParams.Add(new ReqdParam("bucket", "bucket not defined. Backup action requires a target bucket."));
            backupParams.Add(new ReqdParam("folder", "folder not defined. Backup action requires a source folder."));

            Dictionary<string, List<ReqdParam>> validationRules = new Dictionary<string, List<ReqdParam>>();
            validationRules.Add("backup", backupParams);

            // restore required parameters
            List<ReqdParam> restoreParams = new List<ReqdParam>();
            restoreParams.Add(new ReqdParam("bucket", "Restore action requires a source bucket"));

            validationRules.Add("restore", restoreParams);
            return validationRules;
        }

        private static Dictionary<string, Job> addActions()
        {
            Dictionary<string, Job> jobs = new Dictionary<string, Job>();
            jobs.Add("backup", new Backup());
            jobs.Add("restore", new Restore());
            return jobs;
        }


    }
        
}
