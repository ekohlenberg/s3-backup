using System;
using System.Collections.Generic;
using System.Text;
using System.IO;
using System.Reflection;
using System.Data.Common;

namespace s3b
{
    public class Backup : Job
    {

         PersistBase persist = null;

         string bucket = string.Empty;
         string backup_folder = string.Empty;
            int maxRetry = 3;

        public Backup()
        {

        }

        override public bool run(Model args)
        {
            int retry = 0;
            persist = new SqlitePersist(new s3bSqliteTemplate());
            Logger.Persist = persist;

            Logger.info("starting...");

            persist.execCmd("delete from message_log");

            BackupSet bset = BackupSet.factory(args);

            load(bset);

            init(bset);

            folders(bset);

            files(bset);

            newer(bset);

            recon(bset);

            do
            {
                process(bset);
            }
            while (!recon(bset) && (retry++ < maxRetry));

            bset.last_backup_datetime = DateTime.Now;
            persist.update(bset);

            Logger.info("complete.");

            

            return true;
        }
       

        

        private  void setParameters(LocalFolder fldr)
        {
            Config config = Config.getConfig();

            config.setValue("localfolder", fldr.folder_path);
            config.setValue("localfile", fldr.folder_path + "\\*");
            config.setValue("archive.name", fldr.getArchiveName());
        }

        
        // update all prev timestamp to the current timestemp (the last timestamp from the prev run)
         void init(BackupSet bset)
        {

            string sql = @"update local_file set previous_update = current_update where folder_id in (
                           select fldr.id from local_folder fldr where fldr.backup_set_id =$(id))";

            persist.execCmd(bset, sql);

        }
         void load(BackupSet bset)
        {
            Logger.info("loading backup set " + bset.root_folder_path);

            persist.put(bset, "root_folder_path");
        }

         void folders(BackupSet bset)
        {
            string[] dirs = Directory.GetDirectories(bset.root_folder_path);

            Logger.info("loading folders for backup set " + bset.root_folder_path);

            LocalFolder fldr;

            foreach (string d in dirs)
            {
                fldr = childFolderFactory(bset, d);
                persist.get(fldr);
            }

            fldr = rootFolderFactory(bset);

            persist.get(fldr);
        }

        private LocalFolder childFolderFactory(BackupSet bset, string d)
        {
            LocalFolder fldr = new LocalFolder();
            fldr.backup_set_id = bset.id;
            fldr.folder_path = d;
            persist.put(fldr, "folder_path");
            bset.localFolders.Add(fldr.id, fldr);
            return fldr;
        }

        private LocalFolder rootFolderFactory(BackupSet bset)
        {
            LocalFolder fldr = new LocalFolder();
            fldr.backup_set_id = bset.id;
            fldr.folder_path = bset.root_folder_path;
            fldr.recurse = false;
            persist.put(fldr, "folder_path");
            bset.localFolders.Add(fldr.id, fldr);
            fldr.backupSet = bset;
            return fldr;
        }

        delegate void FileCallback(string filename);

        // put all files into the database recursively
         void files(BackupSet bset)
        {
            foreach (LocalFolder fldr in bset.localFolders.Values)
            {
                Logger.info("loading files for folder " + fldr.folder_path);

                FileCallback dcb = (filename) =>
                {
                    FileInfo fi = new FileInfo(filename);
                    LocalFile f = new LocalFile();

                    f.full_path = fi.FullName;
                    f.current_update = fi.LastWriteTime;
                    f.folder_id = fldr.id;

                    persist.put(f, "full_path");
                    fldr.files.Add(f);
                };

                if (fldr.recurse) dirSearch(fldr.folder_path, dcb);

                try
                {
                    foreach (string f in Directory.GetFiles(fldr.folder_path))
                    {
                        dcb(f);
                    }
                }
                catch (Exception x)
                {
                    Logger.error(x);
                }
            }
        }

        private  void dirSearch(string folder_path, FileCallback dcb)
        {
            try
            {
                foreach (string d in Directory.GetDirectories(folder_path))
                {

                    dirSearch(d, dcb);
                }

                foreach (string f in Directory.GetFiles(folder_path))
                {
                    dcb(f);
                }
            }
            catch (Exception x)
            {
                Logger.error(x);
            }
        }

        // select all the parent folders where there exists any file where the current timestamp is newer than the prev timestamp
        // default null prev timestmp to 1/1/1900 
        // insert found parent folders into a work list
         void newer(BackupSet bset)
        {
            /* string sql = @"select distinct fldr.* from local_file f
                             inner join local_folder fldr on
                                 fldr.id = f.folder_id 
                             where
                                 fldr.backup_set_id=$(id) and
                                 (f.current_update > isnull( f.previous_update, convert( datetime, '1900-01-01', 102)) or
                                 (fldr.stage + '.' + fldr.status <> 'upload.complete'))
                             ";
            */
            PersistBase.SelectCallback scb = (rdr) =>
            {
                long id = Convert.ToInt64(rdr["id"]);

                if (bset.localFolders.ContainsKey(id))
                {
                    addExistingWorkingFolder(bset, id);
                }
                else
                {
                    addNewWorkingFolder(bset, rdr, id);
                }
            };

            persist.query(scb, "newer", bset);
        }

        private void addNewWorkingFolder(BackupSet bset, DbDataReader rdr, long id)
        {
            LocalFolder fldr = new LocalFolder();
            persist.autoAssign(rdr, fldr);
            bset.workFolders.Add(fldr);
            bset.localFolders.Add(id, fldr);
            fldr.backupSet = bset;
            Logger.error(fldr.folder_path + " not found in dir listing.");
        }

        private void addExistingWorkingFolder(BackupSet bset, long id)
        {
            LocalFolder fldr = bset.localFolders[id];
            Logger.debug("adding newer folder: " + fldr.folder_path);
            bset.workFolders.Add(fldr);
            fldr.backupSet = bset;
            updateStatus(fldr, "new", "none");
        }

        void process(BackupSet bset)
        {

            foreach (LocalFolder fldr in bset.workFolders)
            // LocalFolder fldr = bset.workFolders[0];
            {
                if (fldr.files.Count > 0)
                {
                    setParameters(fldr);

                    int stageCode = fldr.getStageCode();

                    Logger.info("processing: " + fldr.folder_path);
                    if ((stageCode & (int)LocalFolder.stages.archiveStage) == (int)LocalFolder.stages.archiveStage)
                        archive(fldr);

                    if ((stageCode & (int)LocalFolder.stages.compressStage) == (int)LocalFolder.stages.compressStage)
                        compress(fldr);

                    if ((stageCode & (int)LocalFolder.stages.encryptStage) == (int)LocalFolder.stages.encryptStage)
                        encrypt(fldr);


                    if ((stageCode & (int)LocalFolder.stages.uploadStage) == (int)LocalFolder.stages.uploadStage)
                        upload(fldr);

                    if ((stageCode & (int)LocalFolder.stages.cleanStage) == (int)LocalFolder.stages.cleanStage)
                        clean(fldr);

                }

            }
        }

        private  void archive(LocalFolder fldr)
        {
            Config config = Config.getConfig();

            if (!isStepEnabled("archive")) return;

            Logger.info("archiving: " + fldr.folder_path);

            updateStatus(fldr, MethodBase.GetCurrentMethod().Name, "in_progess");

            int retCode = 0;


            if (fldr.recurse)
            {
                config.setValue("localobject", fldr.folder_path);
                retCode = exec("archive.command", "archive.args");
            }
            else
            {
                foreach (LocalFile f in fldr.files)
                {

                    config.setValue("localfile", f.full_path);
                    config.setValue("localobject", f.full_path);

                    int execCode = exec("archive.command", "archive.args");
                    if (execCode == 1) retCode = 1;
                }
            }


            string completion = "complete";
            if (retCode != 0) completion = "error";

            updateStatus(fldr, MethodBase.GetCurrentMethod().Name, completion);
        }



         void compress(LocalFolder fldr)
        {
            if (!isStepEnabled("compress")) return;

            Logger.info("compressing: " + fldr.folder_path);

            updateStatus(fldr, MethodBase.GetCurrentMethod().Name, "in_progess");

            int retCode = exec("compress.command", "compress.args");
            string completion = "complete";
            if (retCode != 0) completion = "error";



            updateStatus(fldr, MethodBase.GetCurrentMethod().Name, completion);

        }

         void encrypt(LocalFolder fldr)
        {
            Config config = Config.getConfig();

            if (!isStepEnabled("encrypt")) return;

            Logger.info("encrypting: " + fldr.folder_path);

            updateStatus(fldr, MethodBase.GetCurrentMethod().Name, "in_progess");

            string passfile = System.Environment.GetEnvironmentVariable("S3B-PASSFILE");
            config.setValue("passfile", passfile);

            int retCode = exec("encrypt.command", "encrypt.args");
            string completion = "complete";
            if (retCode == 0)
            {
                FileInfo fi = new FileInfo(config.getString("encrypt.clean"));

                fldr.encrypted_file_name = config.getString("encrypt.target");
                fldr.encrypted_file_size = fi.Length;

                persist.update(fldr);
            }
            else
            {
                completion = "error";
            }

            updateStatus(fldr, MethodBase.GetCurrentMethod().Name, completion);
        }

         void upload(LocalFolder fldr)
        {
            Config config = Config.getConfig();

            if (!isStepEnabled("upload")) return;

            Logger.info("uploading: " + fldr.folder_path);

            updateStatus(fldr, MethodBase.GetCurrentMethod().Name, "in_progess");


            config.setValue("uploadtarget", fldr.backupSet.upload_target);

            string completion = "complete";
            if (exec("upload.command", "upload.args") != 0) completion = "error";

            fldr.upload_datetime = DateTime.Now;
            persist.update(fldr);

            updateStatus(fldr, MethodBase.GetCurrentMethod().Name, completion);
        }


         bool recon(BackupSet bset)
        {
            bool result = true;
            if (!isStepEnabled("recon")) return result;

            Logger.info("reconciling...");

            List<string> stdout;
            List<string> stderr;

            exec("recon.command", "recon.args", out stdout, out stderr);

            Dictionary<string, LocalFolder> uploadedFolders = bset.getUploadedFolders();

            foreach (string line in stdout)
            {
                // date
                // time
                // size
                // name
                if (line == null) continue;
                if (line.Trim().Length == 0) continue;

                string[] parts = line.Split(new char[] { ' ' }, StringSplitOptions.RemoveEmptyEntries);

                if (parts.Length != 4) continue;

                DateTime uploadDateTime;
                DateTime.TryParse(parts[0] + " " + parts[1], out uploadDateTime);

                int encryptedFileSize = 0;
                Int32.TryParse(parts[2], out encryptedFileSize);

                string encryptedFileName = parts[3];

                if (uploadedFolders.ContainsKey(encryptedFileName))
                {
                    LocalFolder fldr = uploadedFolders[encryptedFileName];

                    Logger.info("checking " + encryptedFileName);

                    if (encryptedFileSize == fldr.encrypted_file_size)
                    {
                        Logger.info("encrypted file size ok");
                    }
                    else
                    {
                        updateStatus(fldr, "new", "none");
                        Logger.error(fldr.folder_path + " size does not match uploaded " + encryptedFileName);
                        result = false;
                        fldr.backupSet.workFolders.Add(fldr);
                    }
                }
                else
                {
                    Logger.info(encryptedFileName + " not found. skipping.");
                }

            }

            return result;
        }

         int exec(string configCmdName, string configArgName)
        {
            List<string> stdout;
            List<string> stderr;


            int result = exec(configCmdName, configArgName, out stdout, out stderr);

            return result;
        }

        private  int exec(string configCmdName, string configArgName, out List<string> stdout, out List<string> stderr)
        {
            Config config = Config.getConfig();

            string cmd = config.getString(configCmdName);
            string args = config.getString(configArgName);
            ProcExec pe = new ProcExec(cmd, args);
            int result = pe.run(Config.getConfig());

            stdout = pe.stdout;
            stderr = pe.stderr;

            return result;
        }

         void clean(LocalFolder fldr)
        {
            Logger.info("cleaning temp files: " + fldr.folder_path);


            //  clean("archive.clean", cmdParams); // not necessary as gzip removes source fuke


            clean("compress.clean");
            clean("encrypt.clean");


            string completion = "complete";

            updateStatus(fldr, MethodBase.GetCurrentMethod().Name, completion);

        }

         void clean(string configName)
        {
            Config config = Config.getConfig();

            string filename = config.getString(configName);

            if (File.Exists(filename))
            {
                Logger.info("cleaning " + filename);
                File.Delete(filename);
            }
            else
            {
                Logger.info("unable to clean " + filename);
            }

        }

         void updateStatus(LocalFolder fldr, string stage, string status)
        {
            fldr.stage = stage;
            fldr.status = status;
            persist.update(fldr);
        }

         bool isStepEnabled(string stepName)
        {
            bool result = false;

            int enabled = Config.getConfig().getInt(stepName + ".enabled");

            if (enabled == 1) result = true;

            return result;
        }
    }
}
