using System;
using System.Collections.Generic;
using System.IO;
using System.Reflection;
namespace s3b
{
    class Program
    {
        static PersistBase persist = null;

        static string bucket = string.Empty;
        static string backup_folder = string.Empty;

        static int maxRetry = 3;

        public class UsageException : Exception
        {
            public UsageException(string message) : base(message + "\ns3b <backup_folder> <s3_bucket>\n")
            {
            }
        }

        
        static int Main(string[] args)
        {
            int retcode = 0;
            int retry = 0;
            persist = new SqlitePersist(new s3bSqliteTemplate());
            Logger.Persist = persist;

            

            try
            {
                Logger.info("starting...");



                persist.execCmd("delete from message_log");

                BackupSet bset = parse(args);

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

            }
            catch( UsageException u)
            {
                Logger.info(u.Message);
                retcode = 1;
            }
            catch( Exception e)
            {
                Logger.error(e);
                retcode = 1;
            }

            return retcode;
        }

        static BackupSet parse(string[] args)
        {
            if (args.Length != 2) throw new UsageException("Incorrect number of arguments.");
            BackupSet bset = new BackupSet();

            bset.root_folder_path = args[0];
            backup_folder = args[0];

            bset.root_folder_path = Path.GetFullPath(bset.root_folder_path);

            if (!Directory.Exists(bset.root_folder_path)) throw new UsageException("Folder " + bset.root_folder_path + " does not exist.");

            bset.upload_target = args[1];
            bucket = args[1];

            setParameters();

            return bset;
        }

        private static void setParameters(LocalFolder fldr)
        {
            Config.setValue("localfolder", fldr.folder_path);
            Config.setValue("localfile", fldr.folder_path + "\\*");
            Config.setValue("archive.name", fldr.getArchiveName());

        }

        private static void setParameters()
        {
            Config.setValue("temp", Config.getString("s3b.temp"));
            Config.setValue("bucket", bucket);
            Config.setValue("backup_folder", backup_folder);
        }


        // update all prev timestamp to the current timestemp (the last timestamp from the prev run)
        static void init(BackupSet bset)
        {
            
            string sql = @"update local_file set previous_update = current_update where folder_id in (
                           select fldr.id from local_folder fldr where fldr.backup_set_id =$(id))";

            persist.execCmd(bset, sql);
            
        }
        static void load(BackupSet bset)
        {
            Logger.info("loading backup set " + bset.root_folder_path);

            persist.put(bset, "root_folder_path");
        }

        static void folders(BackupSet bset)
        {
            string[] dirs = Directory.GetDirectories(bset.root_folder_path);

            Logger.info("loading folders for backup set " + bset.root_folder_path);

            LocalFolder fldr;

            foreach (string d in dirs)
            {
                fldr = addChildFolder(bset, d);
            }

            fldr = addRootFolder(bset);
        }

        private static LocalFolder addRootFolder(BackupSet bset)
        {
            LocalFolder fldr = new LocalFolder();
            fldr.backup_set_id = bset.id;
            fldr.folder_path = bset.root_folder_path;
            fldr.recurse = false;
            persist.put(fldr, "folder_path");
            bset.localFolders.Add(fldr.id, fldr);
            fldr.backupSet = bset;
            persist.get(fldr);
            return fldr;
        }

        private static LocalFolder addChildFolder(BackupSet bset, string d)
        {
            LocalFolder fldr = new LocalFolder();
            fldr.backup_set_id = bset.id;
            fldr.folder_path = d;
            persist.put(fldr, "folder_path");
            bset.localFolders.Add(fldr.id, fldr);
            fldr.backupSet = bset;
            persist.get(fldr);
            return fldr;
        }

        delegate void FileCallback(string filename);

        // put all files into the database recursively
        static void files(BackupSet bset)
        { 
            foreach(LocalFolder fldr in bset.localFolders.Values)
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
                catch(Exception x)
                {
                    Logger.error(x);
                }
            }
        }

        private static void dirSearch(string folder_path, FileCallback dcb)
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
        static void newer(BackupSet bset)
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
                    LocalFolder fldr = bset.localFolders[id];
                    Logger.debug("adding newer folder: " + fldr.folder_path);
                    bset.workFolders.Add(fldr);
                    fldr.backupSet = bset;
                    updateStatus(fldr, "new", "none");
                }
                else
                {
                    LocalFolder fldr = new LocalFolder();
                    persist.autoAssign(rdr, fldr);
                    bset.workFolders.Add(fldr);
                    bset.localFolders.Add(id, fldr);
                    fldr.backupSet = bset;
                    Logger.error(fldr.folder_path + " not found in dir listing.");
                }
            };

            persist.query(scb, "newer", bset);
        }

        static void process(BackupSet bset)
        {

            foreach ( LocalFolder fldr in bset.workFolders)
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

        private static void archive(LocalFolder fldr)
        {

            if (!isStepEnabled("archive")) return;

            Logger.info("archiving: " + fldr.folder_path);

            updateStatus(fldr, MethodBase.GetCurrentMethod().Name, "in_progess");
            
            int retCode = 0;

                       
            if (fldr.recurse)
            {
                Config.setValue("localobject", fldr.folder_path);
                retCode = exec("archive.command", "archive.args");
            }
            else
            {
                foreach (LocalFile f in fldr.files)
                {
                    
                    Config.setValue("localfile", f.full_path);
                    Config.setValue("localobject", f.full_path);

                    int execCode = exec("archive.command", "archive.args");
                    if (execCode == 1) retCode = 1;
                }
            }
           

            string completion = "complete";
            if (retCode != 0) completion = "error";

            updateStatus(fldr, MethodBase.GetCurrentMethod().Name, completion);
        }

        

        static void compress(LocalFolder fldr)
        {
            if (!isStepEnabled("compress")) return;

            Logger.info("compressing: " + fldr.folder_path);

            updateStatus(fldr, MethodBase.GetCurrentMethod().Name, "in_progess");
            
            int retCode = exec("compress.command", "compress.args");
            string completion = "complete";
            if (retCode != 0) completion = "error";

            

            updateStatus(fldr, MethodBase.GetCurrentMethod().Name, completion);

        }

        static void encrypt(LocalFolder fldr)
        {
            if (!isStepEnabled("encrypt")) return;

            Logger.info("encrypting: " + fldr.folder_path);

            updateStatus(fldr, MethodBase.GetCurrentMethod().Name, "in_progess");

            string passfile = System.Environment.GetEnvironmentVariable("S3B-PASSFILE");
            Config.setValue("passfile", passfile);

            int retCode = exec("encrypt.command", "encrypt.args");
            string completion = "complete";
            if (retCode == 0)
            {
                FileInfo fi = new FileInfo(Config.getString("encrypt.clean"));
                
                fldr.encrypted_file_name = Config.getString("encrypt.target");
                fldr.encrypted_file_size = fi.Length;

                persist.update(fldr);
            }
            else
            {
                completion = "error";
            }

            updateStatus(fldr, MethodBase.GetCurrentMethod().Name, completion);
        }

        static void upload(LocalFolder fldr)
        {
            if (!isStepEnabled("upload")) return;

            Logger.info("uploading: " + fldr.folder_path);

            updateStatus(fldr, MethodBase.GetCurrentMethod().Name, "in_progess");


            Config.setValue("uploadtarget",fldr.backupSet.upload_target);

            string completion = "complete";
            if (exec("upload.command", "upload.args") != 0) completion = "error";

            fldr.upload_datetime = DateTime.Now;
            persist.update(fldr);
            
            updateStatus(fldr, MethodBase.GetCurrentMethod().Name, completion);
        }


        static bool recon(BackupSet bset)
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

                string[] parts = line.Split(new char[] { ' ' },StringSplitOptions.RemoveEmptyEntries);

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
                        if (!bset.workFolders.Contains(fldr))
                        {
                            bset.workFolders.Add(fldr);
                        }
                        Logger.error(fldr.folder_path + " size does not match uploaded " + encryptedFileName);
                        result = false;
                    }
                }
                else
                {
                    Logger.info(encryptedFileName + " not found. skipping.");
                }

            }

            return result;
        }

        static int exec(string configCmdName, string configArgName)
        {
            List<string> stdout;
            List<string> stderr;
           
            
            int result = exec(configCmdName, configArgName,  out stdout, out stderr);

            return result;
        }

        private static int exec(string configCmdName, string configArgName,  out List<string> stdout, out List<string> stderr)
        {
            string cmd = Config.getString(configCmdName);
            string args = Config.getString(configArgName);
            ProcExec pe = new ProcExec(cmd, args);
            int result = pe.run(Config.getSettings());

            stdout = pe.stdout;
            stderr = pe.stderr;

            return result;
        }

        static void clean( LocalFolder fldr)
        {
            Logger.info("cleaning temp files: " + fldr.folder_path);

            
           //  clean("archive.clean", cmdParams); // not necessary as gzip removes source fuke

            
            clean("compress.clean");
            clean("encrypt.clean");
            
            
            string completion = "complete";
            
            updateStatus(fldr, MethodBase.GetCurrentMethod().Name, completion);
            
        }

        static void clean(string configName)
        {
            
            string filename = Config.getString(configName);

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

        static void updateStatus( LocalFolder fldr, string stage, string status)
        {
            fldr.stage = stage;
            fldr.status = status;
            persist.update(fldr);
        }

        static  bool isStepEnabled(string stepName)
        {
            bool result = false;

            int enabled = Config.getInt(stepName + ".enabled");

            if (enabled == 1) result = true;

            return result;
        }
    }
}
