using System;
namespace s3b
{
    public class Restore : Job
    {
        public Restore()
        {
            name = "restore";
            jobs.Add(new Download());
            jobs.Add(new Decrypt());
            jobs.Add(new Decompress());
            jobs.Add(new Expand());
        }


        public override bool run(Model args)
        { 
            bool success = true;

            string o = Config.getConfig().getString("object");

            ProcExec.OutputCallback stdout = (parts) =>
            {
                ObjectInfo objectInfo = ObjectInfo.factory(parts);    

                if ((objectInfo.encrypted_file_name == o ) || (o.Length == 0))
                {
                    Config.getConfig().setValue("encrypted_base_file_name", objectInfo.encrypted_base_file_name);

                    foreach(Job j in jobs)
                    {
                        if (success) { success = j.run(Config.getConfig()); }
                    }

                }
            };

            ProcExec.OutputCallback stderr = (parts) =>
            {
                Logger.error(parts);
            };

            ListObj listObj = new ListObj();
            success = listObj.run(Config.getConfig(), stdout, stderr);
            
            
            return success;
        }
    }
}
